//! 账户命令只向界面提供受限的摘要。Microsoft 授权留在浏览器和 Rust 后端。
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use launcher_core::auth::accounts::{
    AccountError, AccountStore, AccountView, VerifyError, verify_for_launch,
};
use launcher_core::auth::microsoft::{self, AuthError, MinecraftProfile};
use launcher_core::auth::skin::{self, Skin, SkinModel};
use launcher_core::auth::tokens::{MemoryVault, SystemVault, TokenVault, VaultError};
use launcher_core::auth::{LaunchAccount, LaunchAuthority};
use reqwest::Url;
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::state::AppState;

pub struct Accounts {
    pub store: AccountStore,
    vault: Arc<dyn TokenVault>,
    authority: LaunchAuthority,
    changes: Mutex<()>,
    verification: tokio::sync::Mutex<()>,
    pending: Mutex<Option<CancellationToken>>,
    force_onboarding: bool,
    onboarding_marker: Option<PathBuf>,
}

impl Accounts {
    pub fn open(
        data_dir: &Path,
        settings_existed: bool,
        isolated: bool,
        onboarding_marker: Option<PathBuf>,
    ) -> Self {
        let vault: Arc<dyn TokenVault> = if isolated {
            Arc::new(MemoryVault::default())
        } else {
            match SystemVault::open() {
                Ok(vault) => Arc::new(vault),
                Err(error) => {
                    tracing::warn!(%error, "凭据管理器不可用，登录只在本次运行中保留");
                    Arc::new(MemoryVault::default())
                }
            }
        };
        let store = AccountStore::open(data_dir, settings_existed);
        Self {
            store,
            vault,
            authority: LaunchAuthority::default(),
            changes: Mutex::default(),
            verification: tokio::sync::Mutex::default(),
            pending: Mutex::default(),
            force_onboarding: std::env::var_os("MCL_FORCE_ONBOARDING").is_some(),
            onboarding_marker,
        }
    }

    fn onboarding_required(&self, onboarded: bool) -> bool {
        !onboarded
            || self.force_onboarding
            || self.onboarding_marker.as_ref().is_some_and(|path| {
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) => metadata.is_file(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        tracing::warn!(%error, "读取安装引导标记失败");
                        false
                    }
                }
            })
    }

    fn finish_onboarding(&self) -> Result<(), AppError> {
        self.store.finish_onboarding()?;
        if let Some(path) = &self.onboarding_marker {
            match std::fs::remove_file(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => {
                    tracing::warn!(%error, "清除安装引导标记失败");
                    return Err(AppError::new(
                        "accounts.save_failed",
                        "无法保存首次引导进度，请重试",
                    ));
                }
            }
        }
        Ok(())
    }

    fn changes(&self) -> MutexGuard<'_, ()> {
        self.changes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn pending(&self) -> MutexGuard<'_, Option<CancellationToken>> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

pub(crate) fn install_onboarding_marker(portable: bool) -> Option<PathBuf> {
    use tauri::utils::{config::BundleType, platform::bundle_type};

    if portable || !cfg!(windows) || bundle_type() != Some(BundleType::Nsis) {
        return None;
    }
    match std::env::current_exe() {
        Ok(exe) => Some(exe.with_file_name(".lumi-onboarding-pending")),
        Err(error) => {
            tracing::warn!(%error, "定位安装引导标记失败");
            None
        }
    }
}

struct BrowserLogin<'a>(&'a Accounts);

impl Drop for BrowserLogin<'_> {
    fn drop(&mut self) {
        let _ = self.0.pending().take();
    }
}

impl From<AccountError> for AppError {
    fn from(error: AccountError) -> Self {
        let code = match error {
            AccountError::NotFound => "accounts.not_found",
            AccountError::Write(_) => "accounts.save_failed",
        };
        AppError::new(code, error.to_string())
    }
}

impl From<VaultError> for AppError {
    fn from(error: VaultError) -> Self {
        tracing::warn!(%error, "Microsoft 登录凭据读写失败");
        AppError::new("auth.vault", "无法保存或读取登录信息，请重启启动器后重试")
    }
}

impl From<AuthError> for AppError {
    fn from(error: AuthError) -> Self {
        let (code, message) = match error {
            AuthError::Unconfigured | AuthError::AppAccessDenied => {
                ("auth.app_unavailable", "Lumi 的 Minecraft 登录服务暂未开放")
            }
            AuthError::Cancelled | AuthError::Denied => ("auth.cancelled", "已取消 Microsoft 登录"),
            AuthError::Timeout => ("auth.timeout", "登录等待超时，请重试"),
            AuthError::ReloginRequired => ("auth.relogin", "登录已过期，请重新登录 Microsoft 账号"),
            AuthError::XboxProfileRequired => ("auth.xbox_profile", "请先在 Xbox 完成账号设置"),
            AuthError::AccountRestricted => (
                "auth.account_restricted",
                "此账号的权限暂不允许启动 Minecraft，请检查 Microsoft 家庭设置",
            ),
            AuthError::NoJavaLicense => (
                "auth.no_entitlement",
                "此账号没有 Minecraft Java 版游玩权限，请核对所用账号和订阅",
            ),
            AuthError::NoJavaProfile => (
                "auth.no_profile",
                "此账号尚未建立 Java 版档案，请在 Minecraft 官网完成设置后重试",
            ),
            AuthError::CallbackUnavailable => (
                "auth.callback_unavailable",
                "无法接收浏览器的登录结果，请重试",
            ),
            AuthError::InvalidResponse | AuthError::Unavailable => (
                "auth.verify_failed",
                "暂时无法验证 Java 版游玩权限，请联网重试",
            ),
        };
        AppError::new(code, message)
    }
}

impl From<VerifyError> for AppError {
    fn from(error: VerifyError) -> Self {
        match error {
            VerifyError::NoAccount => AppError::new("auth.no_account", error.to_string()),
            VerifyError::Relogin => AppError::new("auth.relogin", error.to_string()),
            VerifyError::Changed => AppError::new("auth.account_changed", error.to_string()),
            VerifyError::Auth(error) => error.into(),
            VerifyError::Vault(error) => error.into(),
            VerifyError::Store(error) => error.into(),
        }
    }
}

pub(crate) struct LaunchSelection {
    id: String,
    pub(crate) player_name: String,
    generation: u64,
}

/// 在接受点击时固定账户和授权代次，后台任务始终验证这次选中的账户。
pub(crate) fn preflight(state: &AppState) -> Result<LaunchSelection, AppError> {
    let accounts = &state.accounts;
    let _changes = accounts.changes();
    let account = accounts.store.current().ok_or(VerifyError::NoAccount)?;
    if !microsoft::configured() {
        return Err(AuthError::Unconfigured.into());
    }
    if accounts.vault.load(&account.id)?.is_none() {
        return Err(VerifyError::Relogin.into());
    }
    Ok(LaunchSelection {
        id: account.id,
        player_name: account.name,
        generation: accounts.authority.generation(),
    })
}

pub(crate) async fn ready(
    state: &AppState,
    selection: &LaunchSelection,
    cancel: &CancellationToken,
) -> Result<LaunchAccount, AppError> {
    let accounts = &state.accounts;
    let _verification = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        lock = accounts.verification.lock() => lock,
    };
    if accounts.authority.generation() != selection.generation {
        return Err(VerifyError::Changed.into());
    }
    let (_, session) = verify_for_launch(
        &accounts.store,
        accounts.vault.as_ref(),
        &selection.id,
        cancel,
    )
    .await?;
    accounts
        .authority
        .authorize(selection.generation, session)
        .map_err(|error| AppError::from(launcher_core::launch::LaunchError::Authorization(error)))
}

/// UUID 仅用于查找世界里的本地统计。启动授权不读这个回退值。
pub(crate) fn current_uuid(state: &AppState) -> uuid::Uuid {
    state
        .accounts
        .store
        .current()
        .map(|account| account.uuid())
        .unwrap_or_else(uuid::Uuid::nil)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountsState {
    pub selected: Option<String>,
    pub accounts: Vec<AccountView>,
    pub onboarding: bool,
    pub microsoft_available: bool,
}

fn accounts_state(state: &AppState) -> AccountsState {
    let file = state.accounts.store.snapshot();
    AccountsState {
        selected: file.current().map(|account| account.id.clone()),
        accounts: file.accounts.iter().map(|account| account.view()).collect(),
        onboarding: state.accounts.onboarding_required(file.onboarded),
        microsoft_available: microsoft::configured(),
    }
}

#[tauri::command]
#[specta::specta]
pub fn accounts_list(state: State<'_, AppState>) -> AccountsState {
    accounts_state(&state)
}

#[tauri::command]
#[specta::specta]
pub fn accounts_skin(state: State<'_, AppState>, account_id: String) -> Option<String> {
    let account = state.accounts.store.get(&account_id)?;
    let skin = account.skin?;
    let bytes = skin::read(state.accounts.store.skins_dir(), &skin.hash)?;
    Some(skin::data_url(&bytes))
}

#[tauri::command]
#[specta::specta]
pub fn accounts_select(state: State<'_, AppState>, account_id: String) -> Result<(), AppError> {
    let accounts = &state.accounts;
    let _changes = accounts.changes();
    accounts.authority.revoke();
    accounts.store.select(&account_id)?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_remove(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<AccountsState, AppError> {
    let accounts = &state.accounts;
    let _changes = accounts.changes();
    accounts.authority.revoke();
    if let Some(pending) = accounts.pending().as_ref() {
        pending.cancel();
    }
    accounts
        .store
        .remove(&account_id, accounts.vault.as_ref())?;
    Ok(accounts_state(&state))
}

#[tauri::command]
#[specta::specta]
pub fn accounts_finish_onboarding(state: State<'_, AppState>) -> Result<(), AppError> {
    state.accounts.finish_onboarding()
}

#[tauri::command]
#[specta::specta]
pub fn accounts_restart_onboarding(state: State<'_, AppState>) -> Result<(), AppError> {
    state.accounts.store.restart_onboarding()?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_microsoft_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AccountsState, AppError> {
    let accounts = &state.accounts;
    if !microsoft::configured() {
        return Err(AuthError::Unconfigured.into());
    }

    let cancel = CancellationToken::new();
    {
        let mut pending = accounts.pending();
        if pending.is_some() {
            return Err(AppError::new(
                "auth.login_in_progress",
                "已有 Microsoft 登录正在进行",
            ));
        }
        *pending = Some(cancel.clone());
    }
    let _browser_login = BrowserLogin(accounts);
    let login = microsoft::begin().await?;
    app.opener()
        .open_url(login.url().as_str(), None::<&str>)
        .map_err(|_| AppError::new("auth.open_browser", "无法打开 Microsoft 登录页面"))?;

    let session = login.finish(&cancel).await?;
    let skin = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        skin = cache_official_skin(session.profile(), accounts.store.skins_dir()) => skin,
    };
    // 既有会话的刷新完成后才登记新登录，避免同一个账户的轮换令牌交叉覆盖。
    let _verification = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        lock = accounts.verification.lock() => lock,
    };
    let _changes = accounts.changes();
    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled.into());
    }
    accounts.authority.revoke();
    accounts
        .store
        .save_session(&session, skin, accounts.vault.as_ref())?;
    Ok(accounts_state(&state))
}

#[tauri::command]
#[specta::specta]
pub fn accounts_microsoft_cancel(state: State<'_, AppState>) -> bool {
    let pending = state.accounts.pending();
    if let Some(cancel) = pending.as_ref() {
        cancel.cancel();
        true
    } else {
        false
    }
}

/// 展示用的官方皮肤下载。只取 Mojang 的公开材质域，不带任何凭据。
async fn cache_official_skin(profile: &MinecraftProfile, directory: &Path) -> Option<Skin> {
    let mut url = Url::parse(profile.skin_url.as_deref()?).ok()?;
    if url.host_str() != Some("textures.minecraft.net")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with("/texture/")
    {
        return None;
    }
    url.set_scheme("https").ok()?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    let mut response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > skin::MAX_SKIN_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    skin::check(&bytes).ok()?;
    let hash = skin::save(directory, &bytes).ok()?;
    let model = if profile.slim {
        SkinModel::Slim
    } else {
        SkinModel::Classic
    };
    Some(Skin { hash, model })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reinstall_reopens_completed_onboarding_until_user_finishes() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = root.path().join("data");
        let install_dir = root.path().join("install");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&install_dir).unwrap();
        let marker = install_dir.join(".lumi-onboarding-pending");
        let accounts_file = data_dir.join("accounts.json");
        std::fs::write(
            &accounts_file,
            r#"{"schemaVersion":1,"selected":null,"accounts":[],"onboarded":true}"#,
        )
        .unwrap();
        let before = std::fs::read(&accounts_file).unwrap();

        let open = |marker| {
            let mut accounts = Accounts::open(&data_dir, true, true, marker);
            accounts.force_onboarding = false;
            accounts
        };
        let accounts = open(Some(marker.clone()));
        assert!(!accounts.onboarding_required(accounts.store.snapshot().onboarded));

        std::fs::write(&marker, b"").unwrap();
        assert!(!open(None).onboarding_required(true));
        assert!(accounts.onboarding_required(accounts.store.snapshot().onboarded));
        assert_eq!(std::fs::read(&accounts_file).unwrap(), before);
        drop(accounts);

        let accounts = open(Some(marker.clone()));
        assert!(accounts.onboarding_required(accounts.store.snapshot().onboarded));
        accounts.finish_onboarding().unwrap();
        assert!(!marker.exists());
        assert!(!accounts.onboarding_required(accounts.store.snapshot().onboarded));
    }
}
