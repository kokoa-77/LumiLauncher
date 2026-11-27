//! 账户摘要与 Minecraft 启动前的官方游玩许可校验。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::microsoft::{self, AuthError, VerifiedSession};
use super::skin::{Skin, SkinModel};
use super::tokens::{TokenVault, VaultError};
use crate::fs::{AtomicWriteError, ReadJsonError, read_json, write_json_atomic};

pub const ACCOUNTS_FILE: &str = "accounts.json";
const SCHEMA: u32 = 3;

/// 账户种类。启动器只保存和接受官方 Microsoft 账户；文件里若有无法识别的未知条目，读取时直接忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AccountKind {
    Microsoft,
    #[serde(other, skip_serializing)]
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct AccountCape {
    pub id: String,
    pub alias: String,
    pub hash: String,
    pub active: bool,
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
    #[serde(default)]
    pub capes: Vec<AccountCape>,
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
    pub capes: Vec<AccountCape>,
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
            capes: self.capes.clone(),
        }
    }

    /// 用于读取本地存档的统计。启动时单独核对官方档案。
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.uuid).unwrap_or_default()
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

    fn retain_supported_accounts(&mut self, path: &Path) {
        let before = self.accounts.len();
        self.accounts
            .retain(|account| account.kind == AccountKind::Microsoft);
        if self.accounts.len() == before {
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
        if let Err(error) = write_json_atomic(path, self) {
            tracing::warn!(%error, "暂未保存清理后的账户文件");
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
    io_locked: Mutex<bool>,
}

impl AccountStore {
    pub fn open(data_dir: &Path, settings_existed: bool) -> Self {
        let path = data_dir.join(ACCOUNTS_FILE);
        let mut io_locked = false;
        let file = match read_json::<AccountsFile>(&path) {
            Ok(Some(mut file)) => {
                file.retain_supported_accounts(&path);
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
                tracing::warn!(%source, "读账户文件失败，暂以只读安全模式启动以免覆写原数据");
                io_locked = path.exists();
                AccountsFile::empty(true)
            }
        };
        Self {
            path,
            skins: data_dir.join("skins"),
            file: Mutex::new(file),
            io_locked: Mutex::new(io_locked),
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

    pub fn active_asset_hashes(&self) -> HashSet<String> {
        let file = self.lock();
        let mut hashes = HashSet::new();
        for account in &file.accounts {
            if let Some(skin) = &account.skin {
                hashes.insert(skin.hash.to_ascii_lowercase());
            }
            for cape in &account.capes {
                hashes.insert(cape.hash.to_ascii_lowercase());
            }
        }
        hashes
    }

    fn save(&self, current: &mut AccountsFile, mut next: AccountsFile) -> Result<(), AccountError> {
        if *self
            .io_locked
            .lock()
            .unwrap_or_else(|error| error.into_inner())
        {
            // 若启动时读取 `accounts.json` 发生瞬时 I/O 失败，先尝试重新读取磁盘合并，绝不盲目覆盖。
            if let Ok(Some(disk)) = read_json::<AccountsFile>(&self.path)
                && current.accounts.is_empty()
                && next.accounts.len() <= 1
            {
                let added = next.accounts.first().cloned();
                next.accounts = disk.accounts;
                if let Some(account) = added
                    && !next
                        .accounts
                        .iter()
                        .any(|existing| existing.uuid == account.uuid)
                {
                    next.accounts.push(account);
                }
            }
            *self
                .io_locked
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = false;
        }
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

    /// 先原子移除账户元信息，再删除系统凭据，避免写盘失败留下失去令牌的僵尸账户。
    pub fn remove(&self, id: &str, vault: &dyn TokenVault) -> Result<(), VerifyError> {
        self.remove_metadata(id)?;
        vault.remove(id)?;
        Ok(())
    }

    pub fn remove_metadata(&self, id: &str) -> Result<(), AccountError> {
        let mut file = self.lock();
        let mut next = file.clone();
        let index = next
            .accounts
            .iter()
            .position(|account| account.id == id)
            .ok_or(AccountError::NotFound)?;
        next.accounts.remove(index);
        if next.selected.as_deref() == Some(id) {
            next.selected = next.accounts.first().map(|account| account.id.clone());
        }
        self.save(&mut file, next)?;
        Ok(())
    }

    /// 登记/更新会话元信息并原子落盘（不持锁调用凭据库），返回 `(account_id, is_new_account)`。
    pub fn upsert_session_metadata(
        &self,
        target_id: &str,
        session: &VerifiedSession,
        skin: Option<Skin>,
        capes: Option<Vec<AccountCape>>,
    ) -> Result<(String, bool), AccountError> {
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
                // 皮肤或披风下载瞬时失败时保留既有本地缓存，不清空外观展示。
                if let Some(skin) = skin {
                    account.skin = Some(skin);
                } else if profile.skin_url.is_none() {
                    account.skin = None;
                }
                if let Some(capes) = capes {
                    account.capes = capes;
                }
                account.id.clone()
            }
            None => {
                let id = target_id.to_owned();
                next.accounts.push(Account {
                    id: id.clone(),
                    kind: AccountKind::Microsoft,
                    name: profile.name.clone(),
                    uuid: profile.uuid.hyphenated().to_string(),
                    skin,
                    capes: capes.unwrap_or_default(),
                    added_at: crate::now_millis() as f64,
                });
                id
            }
        };
        next.selected = Some(id.clone());
        self.save(&mut file, next)?;
        Ok((id, new_account))
    }

    /// 用于兼容旧调用路径；内部会按先落凭据后写元信息的顺序完成保存。
    pub fn save_session(
        &self,
        session: &VerifiedSession,
        skin: Option<Skin>,
        vault: &dyn TokenVault,
    ) -> Result<(), VerifyError> {
        let profile = session.profile();
        let existing_id = {
            let file = self.lock();
            file.accounts
                .iter()
                .find(|account| account.verified_uuid() == Some(profile.uuid))
                .map(|account| account.id.clone())
        };
        let target_id = existing_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
        vault.save(&target_id, session.refresh_token())?;
        let mut file = self.lock();
        let mut next = file.clone();
        let existing = next
            .accounts
            .iter_mut()
            .find(|account| account.id == target_id);
        let new_account = existing.is_none();
        match existing {
            Some(account) => {
                account.name.clone_from(&profile.name);
                if let Some(skin) = skin {
                    account.skin = Some(skin);
                } else if profile.skin_url.is_none() {
                    account.skin = None;
                }
            }
            None => {
                next.accounts.push(Account {
                    id: target_id.clone(),
                    kind: AccountKind::Microsoft,
                    name: profile.name.clone(),
                    uuid: profile.uuid.hyphenated().to_string(),
                    skin,
                    capes: Vec::new(),
                    added_at: crate::now_millis() as f64,
                });
            }
        }
        next.selected = Some(target_id.clone());
        if let Err(error) = self.save(&mut file, next) {
            if new_account && let Err(vault_err) = vault.remove(&target_id) {
                tracing::warn!(%vault_err, "未能清理未登记的 Microsoft 凭据");
            }
            return Err(error.into());
        }
        Ok(())
    }

    pub fn update_profile_assets(
        &self,
        account_id: &str,
        name: &str,
        skin: Option<Skin>,
        explicit_no_skin: bool,
        capes: Option<Vec<AccountCape>>,
    ) -> Result<(), AccountError> {
        let mut file = self.lock();
        let Some(existing) = file
            .accounts
            .iter()
            .find(|account| account.id == account_id)
        else {
            return Err(AccountError::NotFound);
        };
        let next_skin = if explicit_no_skin {
            None
        } else {
            skin.or_else(|| existing.skin.clone())
        };
        let next_capes = capes.unwrap_or_else(|| existing.capes.clone());
        if existing.name == name && existing.skin == next_skin && existing.capes == next_capes {
            return Ok(());
        }
        let mut next = file.clone();
        let account = next
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or(AccountError::NotFound)?;
        account.name = name.to_owned();
        account.skin = next_skin;
        account.capes = next_capes;
        self.save(&mut file, next)
    }

    pub fn save_refreshed_name(
        &self,
        expected: &Account,
        new_name: &str,
    ) -> Result<(), VerifyError> {
        let mut file = self.lock();
        let current = file.current().ok_or(VerifyError::Changed)?;
        if current.id != expected.id {
            return Err(VerifyError::Changed);
        }
        if current.name != new_name {
            let mut next = file.clone();
            let account = next
                .accounts
                .iter_mut()
                .find(|account| account.id == expected.id)
                .ok_or(VerifyError::Changed)?;
            account.name = new_name.to_owned();
            self.save(&mut file, next)?;
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
        let store = AccountStore::open(dir.path(), true);
        assert!(store.snapshot().onboarded);
        assert!(store.current().is_none(), "没有官方账户时不能形成可玩档案");
        assert!(!dir.path().join(ACCOUNTS_FILE).exists());
    }

    #[test]
    fn fresh_install_can_finish_onboarding_without_an_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path(), false);
        assert!(!store.snapshot().onboarded);
        store.finish_onboarding().unwrap();
        let again = AccountStore::open(dir.path(), false);
        assert!(again.snapshot().onboarded);
        assert!(again.current().is_none());
        again.restart_onboarding().unwrap();
        assert!(!AccountStore::open(dir.path(), false).snapshot().onboarded);
    }

    #[test]
    fn unsupported_entries_are_dropped_without_losing_microsoft() {
        let dir = tempfile::tempdir().unwrap();
        let account_file = dir.path().join(ACCOUNTS_FILE);
        std::fs::write(
            &account_file,
            br#"{
                "schemaVersion":1,
                "selected":"unknown",
                "onboarded":true,
                "accounts":[
                    {"id":"unknown","kind":"legacyCustom","name":"Steve","uuid":"invalid","addedAt":1},
                    {"id":"official","kind":"microsoft","name":"Kokoa77","uuid":"54f2906a88dd444ebcb3c68132a30a57","addedAt":3}
                ]
            }"#,
        )
        .unwrap();
        let skin = dir.path().join("skins/player.png");
        std::fs::create_dir_all(skin.parent().unwrap()).unwrap();
        std::fs::write(&skin, b"skin data").unwrap();
        let store = AccountStore::open(dir.path(), false);
        assert_eq!(store.snapshot().accounts.len(), 1);
        assert_eq!(store.snapshot().selected.as_deref(), Some("official"));
        assert!(store.snapshot().onboarded);
        assert_eq!(store.current().unwrap().name, "Kokoa77");
        assert!(matches!(
            store.select("unknown"),
            Err(AccountError::NotFound)
        ));
        let persisted = std::fs::read_to_string(&account_file).unwrap();
        assert!(!persisted.contains("legacyCustom"));
        assert_eq!(std::fs::read(&skin).unwrap(), b"skin data");
        let again = AccountStore::open(dir.path(), false);
        assert_eq!(again.snapshot(), store.snapshot());
    }

    #[tokio::test]
    async fn a_saved_uuid_alone_cannot_authorize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(ACCOUNTS_FILE),
            br#"{"schemaVersion":2,"accounts":[{"id":"x","kind":"microsoft","name":"Player","uuid":"54f2906a88dd444ebcb3c68132a30a57","addedAt":1}]}"#,
        ).unwrap();
        let store = AccountStore::open(dir.path(), false);
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
        let store = AccountStore::open(dir.path(), false);
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
        let store = AccountStore::open(dir.path(), true);
        assert!(store.current().is_none());
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("corrupt"))
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn upsert_keeps_target_id_and_preserves_cached_assets_on_transient_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path(), true);
        let uuid = Uuid::parse_str("54f2906a-88dd-444e-bcb3-c68132a30a57").unwrap();
        let profile = super::super::microsoft::MinecraftProfile {
            uuid,
            name: "Steve".to_owned(),
            skin_url: Some("https://textures.minecraft.net/texture/abcd".to_owned()),
            slim: false,
            capes: Vec::new(),
        };
        let session = VerifiedSession::for_test(profile.clone());
        let cached_skin = Skin {
            hash: "a".repeat(64),
            model: SkinModel::Classic,
        };
        let cached_capes = vec![AccountCape {
            id: "c1".to_owned(),
            alias: "Migrator".to_owned(),
            hash: "b".repeat(64),
            active: true,
        }];
        let (id, is_new) = store
            .upsert_session_metadata(
                "fixed-target-id",
                &session,
                Some(cached_skin.clone()),
                Some(cached_capes.clone()),
            )
            .unwrap();
        assert_eq!(id, "fixed-target-id");
        assert!(is_new);

        // 重登时若皮肤和披风下载瞬时返回 None（但 skin_url 仍存在），应保留既有缓存。
        let (again_id, again_new) = store
            .upsert_session_metadata("ignored-id", &session, None, None)
            .unwrap();
        assert_eq!(again_id, "fixed-target-id");
        assert!(!again_new);
        let saved = store.get("fixed-target-id").unwrap();
        assert_eq!(saved.skin, Some(cached_skin));
        assert_eq!(saved.capes, cached_capes);
    }
}
