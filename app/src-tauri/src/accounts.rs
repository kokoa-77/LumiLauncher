//! 账户命令只向界面提供受限的摘要。Microsoft 授权留在浏览器和 Rust 后端。
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use launcher_core::auth::accounts::{
    AccountCape, AccountError, AccountStore, AccountView, VerifyError,
};
use launcher_core::auth::microsoft::{
    self, AuthError, AuthStage, LoginOptions, MinecraftProfile, VerifiedSession,
};
use launcher_core::auth::skin::{self, Skin, SkinModel};
use launcher_core::auth::tokens::{MemoryVault, SystemVault, TokenVault, VaultError};
use launcher_core::auth::{LaunchAccount, LaunchAuthority};
use launcher_core::redact::Secret;
use reqwest::{Client, Url};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::events::{AccountHealthUpdated, AuthProgressUpdated, emit};
use crate::state::AppState;

const MUTATION_COOLDOWN: Duration = Duration::from_secs(3);
const PREWARM_REUSE_MARGIN: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum VaultKind {
    WindowsCredentialManager,
    MemoryOnly,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AuthSecurityInfo {
    pub vault_kind: VaultKind,
    pub vault_persistent: bool,
    pub pkce_method: String,
    pub response_mode: String,
    pub session_pipe: bool,
    pub callback_mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum AccountHealthStatus {
    Unknown,
    Verifying,
    Ready,
    NeedsRelogin,
    NetworkError,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountHealthView {
    pub status: AccountHealthStatus,
    pub verified_at_ms: Option<f64>,
    pub error_code: Option<String>,
}

impl Default for AccountHealthView {
    fn default() -> Self {
        Self {
            status: AccountHealthStatus::Unknown,
            verified_at_ms: None,
            error_code: None,
        }
    }
}

struct PendingBrowserLogin {
    cancel: CancellationToken,
    url: Option<Secret<String>>,
}

struct WarmSession {
    generation: u64,
    session: VerifiedSession,
}

pub struct Accounts {
    pub store: AccountStore,
    vault: Arc<dyn TokenVault>,
    vault_kind: VaultKind,
    authority: LaunchAuthority,
    changes: Mutex<()>,
    verification: tokio::sync::Mutex<()>,
    pending: Mutex<Option<PendingBrowserLogin>>,
    warm_sessions: Mutex<HashMap<String, WarmSession>>,
    health: Mutex<HashMap<String, AccountHealthView>>,
    last_skin_mutation: Mutex<Option<Instant>>,
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
        let (vault, vault_kind): (Arc<dyn TokenVault>, VaultKind) = if isolated {
            (Arc::new(MemoryVault::default()), VaultKind::MemoryOnly)
        } else {
            match SystemVault::open() {
                Ok(vault) => (Arc::new(vault), VaultKind::WindowsCredentialManager),
                Err(error) => {
                    tracing::warn!(%error, "凭据管理器不可用，登录只在本次运行中保留");
                    (Arc::new(MemoryVault::default()), VaultKind::MemoryOnly)
                }
            }
        };
        let store = AccountStore::open(data_dir, settings_existed);
        Self {
            store,
            vault,
            vault_kind,
            authority: LaunchAuthority::default(),
            changes: Mutex::default(),
            verification: tokio::sync::Mutex::default(),
            pending: Mutex::default(),
            warm_sessions: Mutex::default(),
            health: Mutex::default(),
            last_skin_mutation: Mutex::default(),
            force_onboarding: std::env::var_os("MCL_FORCE_ONBOARDING").is_some(),
            onboarding_marker,
        }
    }

    pub fn clear_warm_sessions(&self) {
        self.warm_sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    fn security_info(&self) -> AuthSecurityInfo {
        AuthSecurityInfo {
            vault_kind: self.vault_kind,
            vault_persistent: self.vault_kind == VaultKind::WindowsCredentialManager,
            pkce_method: "S256".to_owned(),
            response_mode: "form_post".to_owned(),
            session_pipe: true,
            callback_mode: microsoft::active_callback_mode().to_owned(),
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

    fn pending(&self) -> MutexGuard<'_, Option<PendingBrowserLogin>> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn set_health(
        &self,
        app: &AppHandle,
        account_id: &str,
        status: AccountHealthStatus,
        verified_at_ms: Option<f64>,
        error_code: Option<String>,
    ) -> AccountHealthView {
        let view = AccountHealthView {
            status,
            verified_at_ms,
            error_code: error_code.clone(),
        };
        self.health
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(account_id.to_owned(), view.clone());
        emit(
            app,
            AccountHealthUpdated {
                account_id: account_id.to_owned(),
                status,
                verified_at_ms,
                error_code,
            },
        );
        view
    }

    fn take_warm_session(&self, account_id: &str, generation: u64) -> Option<VerifiedSession> {
        let mut guard = self
            .warm_sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let warm = guard.remove(account_id)?;
        if warm.generation == generation && Instant::now() < warm.session.valid_until() {
            Some(warm.session)
        } else {
            None
        }
    }

    fn peek_reusable_warm(&self, account_id: &str, generation: u64) -> Option<VerifiedSession> {
        let guard = self
            .warm_sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let warm = guard.get(account_id)?;
        if warm.generation == generation
            && Instant::now() + PREWARM_REUSE_MARGIN < warm.session.valid_until()
        {
            Some(warm.session.clone())
        } else {
            None
        }
    }

    fn store_warm_session(&self, account_id: &str, generation: u64, session: VerifiedSession) {
        self.warm_sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                account_id.to_owned(),
                WarmSession {
                    generation,
                    session,
                },
            );
    }

    fn check_mutation_cooldown(&self) -> Result<(), AppError> {
        let mut guard = self
            .last_skin_mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        if let Some(prev) = *guard
            && now.duration_since(prev) < MUTATION_COOLDOWN
        {
            return Err(AppError::new(
                "auth.rate_limited",
                "操作过于频繁，请几秒后再试",
            ));
        }
        *guard = Some(now);
        Ok(())
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
            AuthError::Unconfigured => {
                ("auth.app_unavailable", "Lumi 的 Minecraft 登录服务暂未开放")
            }
            AuthError::AppAccessDenied => (
                "auth.app_denied",
                "官方认证服务拒绝了本次访问请求，请确认客户端版本或稍后重试",
            ),
            AuthError::Cancelled | AuthError::Denied => ("auth.cancelled", "已取消 Microsoft 登录"),
            AuthError::Timeout => ("auth.timeout", "登录等待超时，请重试"),
            AuthError::ReloginRequired => ("auth.relogin", "登录已过期，请重新登录 Microsoft 账号"),
            AuthError::XboxProfileRequired => ("auth.xbox_profile", "请先在 Xbox 完成账号设置"),
            AuthError::XboxAccountBanned => (
                "auth.xbox_banned",
                "此 Xbox 账号已被封禁或处于强制限制状态，请前往 Xbox 官网核查",
            ),
            AuthError::XboxRegionBlocked => (
                "auth.xbox_region",
                "当前网络所在地区或账号区域暂不支持 Xbox Live 服务",
            ),
            AuthError::XboxChildAccount => (
                "auth.xbox_child",
                "此账号为未成年人/家庭组受限账号，请在 Microsoft 家庭组批准联机与多人权限",
            ),
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
            AuthError::RateLimited => ("auth.rate_limited", "官方接口请求过于频繁，请稍后再试"),
            AuthError::SkinUploadRejected => (
                "skin.invalid",
                "皮肤图片格式或尺寸不符合 Mojang 官方要求（需 64×64 或 64×32 PNG）",
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

async fn vault_load(
    vault: &Arc<dyn TokenVault>,
    account_id: &str,
) -> Result<Option<Secret<String>>, VaultError> {
    let vault = vault.clone();
    let id = account_id.to_owned();
    tokio::task::spawn_blocking(move || vault.load(&id))
        .await
        .map_err(|_| VaultError::Unsupported)?
}

async fn vault_save(
    vault: &Arc<dyn TokenVault>,
    account_id: &str,
    token: &Secret<String>,
) -> Result<(), VaultError> {
    let vault = vault.clone();
    let id = account_id.to_owned();
    let token = token.clone();
    tokio::task::spawn_blocking(move || vault.save(&id, &token))
        .await
        .map_err(|_| VaultError::Unsupported)?
}

async fn vault_remove(vault: &Arc<dyn TokenVault>, account_id: &str) -> Result<(), VaultError> {
    let vault = vault.clone();
    let id = account_id.to_owned();
    tokio::task::spawn_blocking(move || vault.remove(&id))
        .await
        .map_err(|_| VaultError::Unsupported)?
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
    let generation = accounts.authority.generation();
    if accounts
        .peek_reusable_warm(&account.id, generation)
        .is_none()
        && accounts.vault.load(&account.id)?.is_none()
    {
        return Err(VerifyError::Relogin.into());
    }
    Ok(LaunchSelection {
        id: account.id,
        player_name: account.name,
        generation,
    })
}

pub(crate) async fn ready(
    state: &AppState,
    selection: &LaunchSelection,
    cancel: &CancellationToken,
) -> Result<LaunchAccount, AppError> {
    let accounts = &state.accounts;
    if let Some(warm) = accounts.take_warm_session(&selection.id, selection.generation) {
        return accounts
            .authority
            .authorize(selection.generation, warm)
            .map_err(|error| {
                AppError::from(launcher_core::launch::LaunchError::Authorization(error))
            });
    }

    let _verification = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        lock = accounts.verification.lock() => lock,
    };
    if accounts.authority.generation() != selection.generation {
        return Err(VerifyError::Changed.into());
    }

    let account = accounts.store.current().ok_or(VerifyError::Changed)?;
    if account.id != selection.id {
        return Err(VerifyError::Changed.into());
    }
    let expected_uuid = Uuid::parse_str(&account.uuid).map_err(|_| VerifyError::Relogin)?;
    let refresh_token = vault_load(&accounts.vault, &account.id)
        .await?
        .ok_or(VerifyError::Relogin)?;

    let client = state.auth_http_client();
    let session = microsoft::refresh_with_client(&client, &refresh_token, cancel, |_| {}).await?;
    if session.profile().uuid != expected_uuid {
        return Err(VerifyError::Relogin.into());
    }
    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled.into());
    }
    if accounts.authority.generation() != selection.generation {
        return Err(VerifyError::Changed.into());
    }
    vault_save(&accounts.vault, &account.id, session.refresh_token()).await?;
    let (skin, capes) =
        cache_profile_assets(&client, session.profile(), accounts.store.skins_dir()).await;
    let _ = accounts.store.update_profile_assets(
        &account.id,
        &session.profile().name,
        skin,
        session.profile().skin_url.is_none(),
        capes,
    );
    accounts
        .health
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            account.id.clone(),
            AccountHealthView {
                status: AccountHealthStatus::Ready,
                verified_at_ms: Some(launcher_core::now_millis() as f64),
                error_code: None,
            },
        );
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
    pub security: AuthSecurityInfo,
    pub health: HashMap<String, AccountHealthView>,
}

fn accounts_state(state: &AppState) -> AccountsState {
    let file = state.accounts.store.snapshot();
    let health = state
        .accounts
        .health
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    AccountsState {
        selected: file.current().map(|account| account.id.clone()),
        accounts: file.accounts.iter().map(|account| account.view()).collect(),
        onboarding: state.accounts.onboarding_required(file.onboarded),
        microsoft_available: microsoft::configured(),
        security: state.accounts.security_info(),
        health,
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
pub fn accounts_cape_texture(state: State<'_, AppState>, hash: String) -> Option<String> {
    if !state
        .accounts
        .store
        .active_asset_hashes()
        .contains(&hash.to_ascii_lowercase())
    {
        return None;
    }
    let bytes = skin::read(state.accounts.store.skins_dir(), &hash)?;
    Some(skin::data_url(&bytes))
}

#[tauri::command]
#[specta::specta]
pub fn accounts_select(state: State<'_, AppState>, account_id: String) -> Result<(), AppError> {
    let accounts = &state.accounts;
    let _changes = accounts.changes();
    accounts.authority.revoke();
    accounts.clear_warm_sessions();
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
    {
        let _changes = accounts.changes();
        accounts.authority.revoke();
        accounts.clear_warm_sessions();
        if let Some(pending) = accounts.pending().as_ref() {
            pending.cancel.cancel();
        }
        accounts.store.remove_metadata(&account_id)?;
    }
    vault_remove(&accounts.vault, &account_id).await?;
    accounts
        .health
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&account_id);
    let skins_dir = accounts.store.skins_dir().to_path_buf();
    let keep = accounts.store.active_asset_hashes();
    let _ = tokio::task::spawn_blocking(move || skin::gc_unused(&skins_dir, &keep)).await;
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
        *pending = Some(PendingBrowserLogin {
            cancel: cancel.clone(),
            url: None,
        });
    }
    let _browser_login = BrowserLogin(accounts);
    let login = microsoft::begin_with_options(&LoginOptions {
        lang: state.ui_language(),
    })
    .await?;
    {
        let mut pending = accounts.pending();
        if let Some(slot) = pending.as_mut() {
            slot.url = Some(Secret::new(login.url().as_str().to_owned()));
        }
    }
    emit(
        &app,
        AuthProgressUpdated {
            stage: AuthStage::WaitingBrowser,
            expires_in_secs: Some(login.expires_in_secs()),
        },
    );
    app.opener()
        .open_url(login.url().as_str(), None::<&str>)
        .map_err(|_| AppError::new("auth.open_browser", "无法打开 Microsoft 登录页面"))?;

    let client = state.auth_http_client();
    let app_events = app.clone();
    let refocused = std::sync::atomic::AtomicBool::new(false);
    let session = login
        .finish_with_client(&client, &cancel, move |stage| {
            if !matches!(stage, AuthStage::WaitingBrowser)
                && !refocused.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                crate::window::open_main(&app_events);
            }
            emit(
                &app_events,
                AuthProgressUpdated {
                    stage,
                    expires_in_secs: None,
                },
            );
        })
        .await?;

    let (skin, capes) = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        assets = cache_profile_assets(&client, session.profile(), accounts.store.skins_dir()) => assets,
    };
    let _verification = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        lock = accounts.verification.lock() => lock,
    };
    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled.into());
    }

    let existing_id = {
        let _changes = accounts.changes();
        accounts
            .store
            .snapshot()
            .accounts
            .into_iter()
            .find(|account| account.uuid == session.profile().uuid.hyphenated().to_string())
            .map(|account| account.id)
    };
    let is_new = existing_id.is_none();
    let target_id = existing_id.unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    // 若账户为新登录则直接以 target_id 预存凭据并登记同 ID；出错时仅清理新账号的预存凭据。
    vault_save(&accounts.vault, &target_id, session.refresh_token()).await?;

    let save_res = {
        let _changes = accounts.changes();
        accounts.authority.revoke();
        accounts.clear_warm_sessions();
        accounts
            .store
            .upsert_session_metadata(&target_id, &session, skin, capes)
    };
    let saved_id = match save_res {
        Ok((id, _)) => {
            if id != target_id {
                vault_save(&accounts.vault, &id, session.refresh_token()).await?;
            }
            id
        }
        Err(error) => {
            if is_new {
                let _ = vault_remove(&accounts.vault, &target_id).await;
            }
            return Err(error.into());
        }
    };
    let generation = accounts.authority.generation();
    accounts.store_warm_session(&saved_id, generation, session);
    accounts.set_health(
        &app,
        &saved_id,
        AccountHealthStatus::Ready,
        Some(launcher_core::now_millis() as f64),
        None,
    );
    let skins_dir = accounts.store.skins_dir().to_path_buf();
    let keep = accounts.store.active_asset_hashes();
    let _ = tokio::task::spawn_blocking(move || skin::gc_unused(&skins_dir, &keep)).await;
    Ok(accounts_state(&state))
}

#[tauri::command]
#[specta::specta]
pub fn accounts_microsoft_reopen_browser(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let url = {
        let pending = state.accounts.pending();
        let Some(slot) = pending.as_ref() else {
            return Err(AppError::new(
                "auth.not_pending",
                "当前没有正在等待浏览器授权的登录请求",
            ));
        };
        slot.url
            .as_ref()
            .map(|secret| secret.expose().clone())
            .ok_or_else(|| AppError::new("auth.not_pending", "登录链接尚未准备好，请稍候"))?
    };
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|_| AppError::new("auth.open_browser", "无法打开 Microsoft 登录页面"))
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_microsoft_copy_url(state: State<'_, AppState>) -> Result<(), AppError> {
    let url = {
        let pending = state.accounts.pending();
        let Some(slot) = pending.as_ref() else {
            return Err(AppError::new(
                "auth.not_pending",
                "当前没有正在等待浏览器授权的登录请求",
            ));
        };
        slot.url
            .as_ref()
            .map(|secret| secret.expose().clone())
            .ok_or_else(|| AppError::new("auth.not_pending", "登录链接尚未准备好，请稍候"))?
    };
    if !url.is_ascii() {
        return Err(AppError::new("auth.copy_failed", "登录链接包含无效字符"));
    }
    tokio::task::spawn_blocking(move || copy_ascii_to_clipboard(&url))
        .await
        .map_err(|_| AppError::new("auth.copy_failed", "复制登录链接失败"))?
}

fn copy_ascii_to_clipboard(text: &str) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = Command::new("clip")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| AppError::new("auth.copy_failed", "无法调用系统剪贴板"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|_| AppError::new("auth.copy_failed", "写入剪贴板失败"))?;
        }
        let status = child
            .wait()
            .map_err(|_| AppError::new("auth.copy_failed", "等待剪贴板完成失败"))?;
        if !status.success() {
            return Err(AppError::new("auth.copy_failed", "写入剪贴板失败"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err(AppError::new(
            "auth.copy_failed",
            "当前系统不支持后端直接复制",
        ))
    }
}

#[tauri::command]
#[specta::specta]
pub fn accounts_microsoft_cancel(state: State<'_, AppState>) -> bool {
    let pending = state.accounts.pending();
    if let Some(slot) = pending.as_ref() {
        slot.cancel.cancel();
        true
    } else {
        false
    }
}

async fn ensure_verified_session(
    app: &AppHandle,
    state: &AppState,
    account_id: &str,
    cancel: &CancellationToken,
) -> Result<VerifiedSession, AppError> {
    let accounts = &state.accounts;
    let generation = accounts.authority.generation();
    if let Some(session) = accounts.peek_reusable_warm(account_id, generation) {
        return Ok(session);
    }
    let _verification = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled.into()),
        lock = accounts.verification.lock() => lock,
    };
    let generation = accounts.authority.generation();
    if let Some(session) = accounts.peek_reusable_warm(account_id, generation) {
        return Ok(session);
    }
    let account = accounts
        .store
        .get(account_id)
        .ok_or(AccountError::NotFound)?;
    let expected_uuid = Uuid::parse_str(&account.uuid).map_err(|_| VerifyError::Relogin)?;
    let Some(token) = vault_load(&accounts.vault, &account.id).await? else {
        accounts.set_health(
            app,
            &account.id,
            AccountHealthStatus::NeedsRelogin,
            None,
            Some("auth.relogin".to_owned()),
        );
        return Err(VerifyError::Relogin.into());
    };
    accounts.set_health(app, &account.id, AccountHealthStatus::Verifying, None, None);
    let client = state.auth_http_client();
    let session = match microsoft::refresh_with_client(&client, &token, cancel, |_| {}).await {
        Ok(session) => session,
        Err(err) => {
            let app_err = AppError::from(err);
            let status = if matches!(
                err,
                AuthError::ReloginRequired
                    | AuthError::Denied
                    | AuthError::NoJavaLicense
                    | AuthError::NoJavaProfile
                    | AuthError::XboxProfileRequired
                    | AuthError::XboxAccountBanned
                    | AuthError::XboxRegionBlocked
                    | AuthError::XboxChildAccount
                    | AuthError::AccountRestricted
            ) {
                AccountHealthStatus::NeedsRelogin
            } else {
                AccountHealthStatus::NetworkError
            };
            accounts.set_health(app, &account.id, status, None, Some(app_err.code.clone()));
            return Err(app_err);
        }
    };
    if session.profile().uuid != expected_uuid {
        accounts.set_health(
            app,
            &account.id,
            AccountHealthStatus::NeedsRelogin,
            None,
            Some("auth.relogin".to_owned()),
        );
        return Err(VerifyError::Relogin.into());
    }
    vault_save(&accounts.vault, &account.id, session.refresh_token()).await?;
    let (skin, capes) =
        cache_profile_assets(&client, session.profile(), accounts.store.skins_dir()).await;
    let _ = accounts.store.update_profile_assets(
        &account.id,
        &session.profile().name,
        skin,
        session.profile().skin_url.is_none(),
        capes,
    );
    accounts.store_warm_session(&account.id, generation, session.clone());
    Ok(accounts.set_health_ready(app, &account.id)).map(|_| session)
}

impl Accounts {
    fn set_health_ready(&self, app: &AppHandle, account_id: &str) -> AccountHealthView {
        self.set_health(
            app,
            account_id,
            AccountHealthStatus::Ready,
            Some(launcher_core::now_millis() as f64),
            None,
        )
    }
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_prewarm(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<AccountHealthView, AppError> {
    if !microsoft::configured() {
        return Err(AuthError::Unconfigured.into());
    }
    let target = match account_id {
        Some(id) => id,
        None => {
            state
                .accounts
                .store
                .current()
                .ok_or(VerifyError::NoAccount)?
                .id
        }
    };
    let cancel = CancellationToken::new();
    let _ = ensure_verified_session(&app, &state, &target, &cancel).await?;
    let health = state
        .accounts
        .health
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&target)
        .cloned()
        .unwrap_or_default();
    Ok(health)
}

async fn apply_profile_update(
    state: &AppState,
    client: &Client,
    account_id: &str,
    profile: &MinecraftProfile,
) -> Result<AccountsState, AppError> {
    let (skin, capes) =
        cache_profile_assets(client, profile, state.accounts.store.skins_dir()).await;
    state.accounts.store.update_profile_assets(
        account_id,
        &profile.name,
        skin,
        profile.skin_url.is_none(),
        capes,
    )?;
    let skins_dir = state.accounts.store.skins_dir().to_path_buf();
    let keep = state.accounts.store.active_asset_hashes();
    let _ = tokio::task::spawn_blocking(move || skin::gc_unused(&skins_dir, &keep)).await;
    Ok(accounts_state(state))
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_skin_refresh(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<AccountsState, AppError> {
    state.accounts.check_mutation_cooldown()?;
    let cancel = CancellationToken::new();
    let session = ensure_verified_session(&app, &state, &account_id, &cancel).await?;
    let client = state.auth_http_client();
    let profile = microsoft::fetch_profile(&client, session.game_token(), &cancel).await?;
    apply_profile_update(&state, &client, &account_id, &profile).await
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_skin_upload(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    model: Option<SkinModel>,
) -> Result<Option<AccountsState>, AppError> {
    let dialog_app = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("选择 Minecraft 皮肤图片（PNG）")
            .add_filter("PNG 图片", &["png"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| AppError::new("app.internal", "无法打开文件选择器"))?;
    let Some(file) = picked else {
        return Ok(None);
    };
    let path = file
        .into_path()
        .map_err(|_| AppError::new("skin.invalid", "只支持本机 PNG 皮肤文件"))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::new("skin.invalid", "无法读取选中的皮肤文件"))?;
    let detected =
        skin::check(&bytes).map_err(|err| AppError::new("skin.invalid", err.to_string()))?;
    let chosen_model = model.unwrap_or(detected);

    state.accounts.check_mutation_cooldown()?;
    let cancel = CancellationToken::new();
    let session = ensure_verified_session(&app, &state, &account_id, &cancel).await?;
    let client = state.auth_http_client();
    let profile = microsoft::upload_skin(
        &client,
        session.game_token(),
        &bytes,
        chosen_model == SkinModel::Slim,
        &cancel,
    )
    .await?;
    Ok(Some(
        apply_profile_update(&state, &client, &account_id, &profile).await?,
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_skin_reset(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<AccountsState, AppError> {
    state.accounts.check_mutation_cooldown()?;
    let cancel = CancellationToken::new();
    let session = ensure_verified_session(&app, &state, &account_id, &cancel).await?;
    let client = state.auth_http_client();
    let profile = microsoft::reset_skin(&client, session.game_token(), &cancel).await?;
    apply_profile_update(&state, &client, &account_id, &profile).await
}

#[tauri::command]
#[specta::specta]
pub async fn accounts_cape_set(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    cape_id: Option<String>,
) -> Result<AccountsState, AppError> {
    if let Some(ref target_id) = cape_id {
        let account = state
            .accounts
            .store
            .get(&account_id)
            .ok_or(AccountError::NotFound)?;
        if !account.capes.iter().any(|cape| &cape.id == target_id) {
            return Err(AppError::new("skin.invalid", "该账号未持有此披风"));
        }
    }
    state.accounts.check_mutation_cooldown()?;
    let cancel = CancellationToken::new();
    let session = ensure_verified_session(&app, &state, &account_id, &cancel).await?;
    let client = state.auth_http_client();
    let profile =
        microsoft::set_active_cape(&client, session.game_token(), cape_id.as_deref(), &cancel)
            .await?;
    apply_profile_update(&state, &client, &account_id, &profile).await
}

/// 展示用的官方皮肤和披风下载。使用 DoH + 代理认证客户端，只取 Mojang 公开材质域，不带凭据。
async fn cache_profile_assets(
    client: &Client,
    profile: &MinecraftProfile,
    directory: &Path,
) -> (Option<Skin>, Option<Vec<AccountCape>>) {
    let skin = match profile.skin_url.as_deref() {
        Some(raw_url) => download_official_texture(client, raw_url)
            .await
            .and_then(|bytes| {
                skin::check(&bytes).ok()?;
                let hash = skin::save(directory, &bytes).ok()?;
                let model = if profile.slim {
                    SkinModel::Slim
                } else {
                    SkinModel::Classic
                };
                Some(Skin { hash, model })
            }),
        None => None,
    };

    let mut capes = Vec::with_capacity(profile.capes.len());
    let mut cape_failed = false;
    for cape in &profile.capes {
        let Some(bytes) = download_official_texture(client, &cape.url).await else {
            cape_failed = true;
            break;
        };
        if skin::check_cape(&bytes).is_err() {
            cape_failed = true;
            break;
        }
        let Ok(hash) = skin::save(directory, &bytes) else {
            cape_failed = true;
            break;
        };
        capes.push(AccountCape {
            id: cape.id.clone(),
            alias: cape.alias.clone(),
            hash,
            active: cape.active,
        });
    }
    let capes_opt = if profile.capes.is_empty() {
        Some(Vec::new())
    } else if cape_failed {
        None
    } else {
        Some(capes)
    };
    (skin, capes_opt)
}

async fn download_official_texture(client: &Client, raw_url: &str) -> Option<Vec<u8>> {
    let mut url = Url::parse(raw_url).ok()?;
    if url.host_str() != Some("textures.minecraft.net")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with("/texture/")
    {
        return None;
    }
    url.set_scheme("https").ok()?;
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
    Some(bytes)
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
