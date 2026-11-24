//! 账户摘要与 Minecraft 启动前的官方游玩许可校验。
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::legacy_name_uuid;
use super::microsoft::{self, AuthError, VerifiedSession};
use super::skin::{Skin, SkinModel};
use super::tokens::{TokenVault, VaultError};
use crate::fs::{AtomicWriteError, ReadJsonError, read_json, write_json_atomic};

pub const ACCOUNTS_FILE: &str = "accounts.json";
const SCHEMA: u32 = 3;

/// 账户种类。只有 Microsoft 账户能启动游戏；另外两种是早期本地预览版留下的，
/// 读到旧文件时直接移除（[`AccountsFile::remove_retired_accounts`]），从不写回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AccountKind {
    /// 早期预览版的用户名账户（旧文件里写作 `offline`）：读到即移除
    #[serde(rename = "offline")]
    RetiredUsername,
    /// 早期预览版的皮肤站账户（旧文件里写作 `skinSite`）：读到即移除，并清掉 Lumi 自己存的旧凭据
    #[serde(rename = "skinSite")]
    RetiredSkinSite,
    Microsoft,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    kind: AccountKind,
    pub name: String,
    pub uuid: String,
    #[serde(default)]
    pub skin: Option<Skin>,
    pub added_at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub name: String,
    pub uuid: String,
    pub has_skin: bool,
    pub skin_model: SkinModel,
    pub skin_hash: Option<String>,
}

impl Account {
    pub fn view(&self) -> AccountView {
        AccountView {
            id: self.id.clone(),
            name: self.name.clone(),
            uuid: self.uuid.clone(),
            has_skin: self.skin.is_some(),
            skin_model: self
                .skin
                .as_ref()
                .map(|skin| skin.model)
                .unwrap_or_default(),
            skin_hash: self.skin.as_ref().map(|skin| skin.hash.clone()),
        }
    }

    /// 用于读取本地存档的统计。启动时单独核对官方档案。
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.uuid).unwrap_or_else(|_| legacy_name_uuid(&self.name))
    }

    fn verified_uuid(&self) -> Option<Uuid> {
        Uuid::parse_str(&self.uuid).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsFile {
    pub schema_version: u32,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default = "yes")]
    pub onboarded: bool,
}

fn yes() -> bool {
    true
}

impl AccountsFile {
    fn empty(onboarded: bool) -> Self {
        Self {
            schema_version: SCHEMA,
            selected: None,
            accounts: Vec::new(),
            onboarded,
        }
    }

    fn remove_retired_accounts(&mut self, path: &Path, vault: &dyn TokenVault) {
        let mut changed = false;
        let mut credentials_cleaned = true;
        self.accounts.retain(|account| {
            if account.kind == AccountKind::Microsoft {
                return true;
            }
            changed = true;
            if account.kind == AccountKind::RetiredSkinSite
                && let Err(error) = vault.remove_legacy(&account.id)
            {
                tracing::warn!(%error, "暂未清理旧皮肤站凭据");
                credentials_cleaned = false;
            }
            false
        });
        if !changed {
            return;
        }
        if !self
            .accounts
            .iter()
            .any(|account| Some(&account.id) == self.selected.as_ref())
        {
            self.selected = self.accounts.first().map(|account| account.id.clone());
        }
        self.schema_version = self.schema_version.max(SCHEMA);
        if credentials_cleaned && let Err(error) = write_json_atomic(path, self) {
            tracing::warn!(%error, "暂未保存升级后的账户文件");
        }
    }

    pub fn current(&self) -> Option<&Account> {
        self.selected
            .as_deref()
            .and_then(|id| self.accounts.iter().find(|account| account.id == id))
            .or_else(|| self.accounts.first())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("找不到这个账户")]
    NotFound,
    #[error("保存账户失败")]
    Write(#[from] AtomicWriteError),
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("请先使用 Microsoft 账号登录")]
    NoAccount,
    #[error("登录信息已失效，请重新登录 Microsoft 账号")]
    Relogin,
    #[error("启动账户已更改，请重新启动")]
    Changed,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Store(#[from] AccountError),
}

pub struct AccountStore {
    path: PathBuf,
    skins: PathBuf,
    file: Mutex<AccountsFile>,
}

impl AccountStore {
    pub fn open(data_dir: &Path, settings_existed: bool, vault: &dyn TokenVault) -> Self {
        let path = data_dir.join(ACCOUNTS_FILE);
        let file = match read_json::<AccountsFile>(&path) {
            Ok(Some(mut file)) => {
                file.remove_retired_accounts(&path, vault);
                file
            }
            Ok(None) => AccountsFile::empty(settings_existed),
            Err(ReadJsonError::Parse { source, .. }) => {
                let backup =
                    path.with_file_name(format!("{ACCOUNTS_FILE}.corrupt-{}", crate::now_millis()));
                tracing::warn!(%source, "账户文件损坏，备份后重新开始");
                if let Err(error) = std::fs::rename(&path, &backup) {
                    tracing::warn!(%error, "备份损坏的账户文件失败");
                }
                AccountsFile::empty(true)
            }
            Err(ReadJsonError::Io { source, .. }) => {
                tracing::warn!(%source, "读账户文件失败");
                AccountsFile::empty(true)
            }
        };
        Self {
            path,
            skins: data_dir.join("skins"),
            file: Mutex::new(file),
        }
    }

    fn lock(&self) -> MutexGuard<'_, AccountsFile> {
        self.file.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn skins_dir(&self) -> &Path {
        &self.skins
    }

    pub fn snapshot(&self) -> AccountsFile {
        self.lock().clone()
    }

    pub fn current(&self) -> Option<Account> {
        self.lock().current().cloned()
    }

    pub fn get(&self, id: &str) -> Option<Account> {
        self.lock()
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
    }

    fn save(&self, current: &mut AccountsFile, mut next: AccountsFile) -> Result<(), AccountError> {
        next.schema_version = SCHEMA;
        write_json_atomic(&self.path, &next)?;
        *current = next;
        Ok(())
    }

    pub fn select(&self, id: &str) -> Result<(), AccountError> {
        let mut file = self.lock();
        if !file.accounts.iter().any(|account| account.id == id) {
            return Err(AccountError::NotFound);
        }
        let mut next = file.clone();
        next.selected = Some(id.to_owned());
        self.save(&mut file, next)
    }

    pub fn remove(&self, id: &str, vault: &dyn TokenVault) -> Result<(), VerifyError> {
        let mut file = self.lock();
        let mut next = file.clone();
        let index = next
            .accounts
            .iter()
            .position(|account| account.id == id)
            .ok_or(AccountError::NotFound)?;
        next.accounts.remove(index);
        vault.remove(id)?;
        if next.selected.as_deref() == Some(id) {
            next.selected = next.accounts.first().map(|account| account.id.clone());
        }
        self.save(&mut file, next)?;
        Ok(())
    }

    pub fn save_session(
        &self,
        session: &VerifiedSession,
        skin: Option<Skin>,
        vault: &dyn TokenVault,
    ) -> Result<(), VerifyError> {
        let profile = session.profile();
        let mut file = self.lock();
        let mut next = file.clone();
        let existing = next
            .accounts
            .iter_mut()
            .find(|account| account.verified_uuid() == Some(profile.uuid));
        let new_account = existing.is_none();
        let id = match existing {
            Some(account) => {
                account.name.clone_from(&profile.name);
                account.skin = skin;
                account.id.clone()
            }
            None => {
                let id = Uuid::new_v4().simple().to_string();
                next.accounts.push(Account {
                    id: id.clone(),
                    kind: AccountKind::Microsoft,
                    name: profile.name.clone(),
                    uuid: profile.uuid.hyphenated().to_string(),
                    skin,
                    added_at: crate::now_millis() as f64,
                });
                id
            }
        };
        next.selected = Some(id.clone());
        vault.save(&id, session.refresh_token())?;
        if let Err(error) = self.save(&mut file, next) {
            if new_account && let Err(error) = vault.remove(&id) {
                tracing::warn!(%error, "未能清理未登记的 Microsoft 凭据");
            }
            return Err(error.into());
        }
        Ok(())
    }

    fn save_refreshed(
        &self,
        expected: &Account,
        session: &VerifiedSession,
        vault: &dyn TokenVault,
    ) -> Result<(), VerifyError> {
        let mut file = self.lock();
        let current = file.current().ok_or(VerifyError::Changed)?;
        if current.id != expected.id || current.verified_uuid() != Some(session.profile().uuid) {
            return Err(VerifyError::Changed);
        }
        vault.save(&expected.id, session.refresh_token())?;
        if current.name != session.profile().name {
            let mut next = file.clone();
            let account = next
                .accounts
                .iter_mut()
                .find(|account| account.id == expected.id)
                .ok_or(VerifyError::Changed)?;
            account.name.clone_from(&session.profile().name);
            self.save(&mut file, next)?;
        }
        Ok(())
    }

    pub fn finish_onboarding(&self) -> Result<(), AccountError> {
        self.set_onboarding(true)
    }

    pub fn restart_onboarding(&self) -> Result<(), AccountError> {
        self.set_onboarding(false)
    }

    fn set_onboarding(&self, onboarded: bool) -> Result<(), AccountError> {
        let mut file = self.lock();
        let mut next = file.clone();
        next.onboarded = onboarded;
        self.save(&mut file, next)
    }
}

pub async fn verify_for_launch(
    store: &AccountStore,
    vault: &dyn TokenVault,
    account_id: &str,
    cancel: &CancellationToken,
) -> Result<(String, VerifiedSession), VerifyError> {
    let account = store.current().ok_or(VerifyError::Changed)?;
    if account.id != account_id {
        return Err(VerifyError::Changed);
    }
    let uuid = account.verified_uuid().ok_or(VerifyError::Relogin)?;
    let token = vault.load(&account.id)?.ok_or(VerifyError::Relogin)?;
    let session = microsoft::refresh(&token, cancel).await?;
    if session.profile().uuid != uuid {
        return Err(VerifyError::Relogin);
    }
    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled.into());
    }
    store.save_refreshed(&account, &session, vault)?;
    Ok((account.id, session))
}

#[cfg(test)]
mod tests {
    use super::super::tokens::MemoryVault;
    use super::*;

    #[test]
    fn reopening_old_settings_does_not_create_a_playable_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path(), true, &MemoryVault::default());
        assert!(store.snapshot().onboarded);
        assert!(store.current().is_none(), "旧玩家名不能形成可玩的账户");
        assert!(!dir.path().join(ACCOUNTS_FILE).exists());
    }

    #[test]
    fn fresh_install_can_finish_onboarding_without_an_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path(), false, &MemoryVault::default());
        assert!(!store.snapshot().onboarded);
        store.finish_onboarding().unwrap();
        let again = AccountStore::open(dir.path(), false, &MemoryVault::default());
        assert!(again.snapshot().onboarded);
        assert!(again.current().is_none());
        again.restart_onboarding().unwrap();
        assert!(
            !AccountStore::open(dir.path(), false, &MemoryVault::default())
                .snapshot()
                .onboarded
        );
    }

    #[test]
    fn old_profiles_are_removed_without_losing_microsoft_on_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let account_file = dir.path().join(ACCOUNTS_FILE);
        std::fs::write(
            &account_file,
            br#"{
                "schemaVersion":1,
                "selected":"legacy",
                "onboarded":true,
                "accounts":[
                    {"id":"legacy","kind":"offline","name":"Steve","uuid":"old","addedAt":1},
                    {"id":"skin","kind":"skinSite","name":"Alex","uuid":"old-site","addedAt":2,
                     "login":"private-example","server":{"root":"https://example.invalid","name":"Old"}},
                    {"id":"official","kind":"microsoft","name":"Kokoa77","uuid":"54f2906a88dd444ebcb3c68132a30a57","addedAt":3}
                ]
            }"#,
        )
        .unwrap();
        let skin = dir.path().join("skins/player.png");
        std::fs::create_dir_all(skin.parent().unwrap()).unwrap();
        std::fs::write(&skin, b"skin data").unwrap();
        let store = AccountStore::open(dir.path(), false, &MemoryVault::default());
        assert_eq!(store.snapshot().accounts.len(), 1);
        assert_eq!(store.snapshot().selected.as_deref(), Some("official"));
        assert!(store.snapshot().onboarded);
        assert_eq!(store.current().unwrap().name, "Kokoa77");
        assert!(matches!(
            store.select("legacy"),
            Err(AccountError::NotFound)
        ));
        assert!(matches!(store.select("skin"), Err(AccountError::NotFound)));
        let persisted = std::fs::read_to_string(&account_file).unwrap();
        assert!(!persisted.contains("private-example"));
        assert!(!persisted.contains("skinSite"));
        assert!(!persisted.contains("offline"));
        assert_eq!(std::fs::read(&skin).unwrap(), b"skin data");
        let again = AccountStore::open(dir.path(), false, &MemoryVault::default());
        assert_eq!(again.snapshot(), store.snapshot());
    }

    #[tokio::test]
    async fn a_saved_uuid_alone_cannot_authorize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(ACCOUNTS_FILE),
            br#"{"schemaVersion":2,"accounts":[{"id":"x","kind":"microsoft","name":"Player","uuid":"54f2906a88dd444ebcb3c68132a30a57","addedAt":1}]}"#,
        ).unwrap();
        let store = AccountStore::open(dir.path(), false, &MemoryVault::default());
        assert!(matches!(
            verify_for_launch(
                &store,
                &MemoryVault::default(),
                "x",
                &CancellationToken::new()
            )
            .await,
            Err(VerifyError::Relogin)
        ));
    }

    #[tokio::test]
    async fn a_launch_requested_by_another_account_cannot_follow_selection() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(ACCOUNTS_FILE),
            br#"{"schemaVersion":2,"selected":"a","accounts":[{"id":"a","kind":"microsoft","name":"Before","uuid":"54f2906a88dd444ebcb3c68132a30a57","addedAt":1},{"id":"b","kind":"microsoft","name":"After","uuid":"6569e28bf07345f4963818946f5a789f","addedAt":2}]}"#,
        ).unwrap();
        let store = AccountStore::open(dir.path(), false, &MemoryVault::default());
        let pending_account = store.current().unwrap().id;
        store.select("b").unwrap();
        assert_ne!(store.current().unwrap().id, pending_account);
        assert!(matches!(
            verify_for_launch(
                &store,
                &MemoryVault::default(),
                &pending_account,
                &CancellationToken::new()
            )
            .await,
            Err(VerifyError::Changed)
        ));
    }
    #[test]
    fn corrupt_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ACCOUNTS_FILE), b"{oops").unwrap();
        let store = AccountStore::open(dir.path(), true, &MemoryVault::default());
        assert!(store.current().is_none());
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("corrupt"))
            .count();
        assert_eq!(backups, 1);
    }
}
