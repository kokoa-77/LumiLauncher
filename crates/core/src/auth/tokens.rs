//! Microsoft 刷新令牌只存玩家自己的系统凭据库，按 Lumi 账户隔离。
use std::collections::HashMap;
use std::sync::Mutex;

use crate::redact::Secret;

const SERVICE: &str = "Lumi Launcher";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("读写 Windows 凭据管理器失败")]
    Store(#[source] keyring_core::Error),
    #[error("当前系统的凭据管理器不可用")]
    Unsupported,
}

pub trait TokenVault: Send + Sync {
    fn load(&self, account_id: &str) -> Result<Option<Secret<String>>, VaultError>;
    fn save(&self, account_id: &str, token: &Secret<String>) -> Result<(), VaultError>;
    fn remove(&self, account_id: &str) -> Result<(), VaultError>;
    fn remove_legacy(&self, account_id: &str) -> Result<(), VaultError>;
}

pub struct SystemVault {
    store: std::sync::Arc<keyring_core::CredentialStore>,
}

impl SystemVault {
    #[cfg(windows)]
    pub fn open() -> Result<Self, VaultError> {
        let store = windows_native_keyring_store::Store::new().map_err(VaultError::Store)?;
        Ok(Self { store })
    }

    #[cfg(not(windows))]
    pub fn open() -> Result<Self, VaultError> {
        Err(VaultError::Unsupported)
    }

    fn entry(&self, namespace: &str, account_id: &str) -> Result<keyring_core::Entry, VaultError> {
        self.store
            .build(SERVICE, &format!("{namespace}:{account_id}"), None)
            .map_err(VaultError::Store)
    }

    fn remove_entry(&self, namespace: &str, account_id: &str) -> Result<(), VaultError> {
        match self.entry(namespace, account_id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(VaultError::Store(error)),
        }
    }
}

impl TokenVault for SystemVault {
    fn load(&self, account_id: &str) -> Result<Option<Secret<String>>, VaultError> {
        match self.entry("microsoft", account_id)?.get_password() {
            Ok(token) => Ok(Some(Secret::new(token))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(VaultError::Store(error)),
        }
    }

    fn save(&self, account_id: &str, token: &Secret<String>) -> Result<(), VaultError> {
        self.entry("microsoft", account_id)?
            .set_password(token.expose())
            .map_err(VaultError::Store)
    }

    fn remove(&self, account_id: &str) -> Result<(), VaultError> {
        self.remove_entry("microsoft", account_id)
    }

    fn remove_legacy(&self, account_id: &str) -> Result<(), VaultError> {
        // 旧账户 ID 由 Lumi 生成，避免用导入的数据指向任意凭据。
        if uuid::Uuid::parse_str(account_id).is_ok() {
            self.remove_entry("skin-site", account_id)?;
        }
        Ok(())
    }
}

/// 系统凭据库不可用时，令牌仅在这一进程的内存里存放。
#[derive(Default)]
pub struct MemoryVault {
    tokens: Mutex<HashMap<String, Secret<String>>>,
}

impl TokenVault for MemoryVault {
    fn load(&self, account_id: &str) -> Result<Option<Secret<String>>, VaultError> {
        let tokens = self
            .tokens
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Ok(tokens.get(account_id).cloned())
    }

    fn save(&self, account_id: &str, token: &Secret<String>) -> Result<(), VaultError> {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        tokens.insert(account_id.to_owned(), token.clone());
        Ok(())
    }

    fn remove(&self, account_id: &str) -> Result<(), VaultError> {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        tokens.remove(account_id);
        Ok(())
    }

    fn remove_legacy(&self, _account_id: &str) -> Result<(), VaultError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_one_account_keeps_the_other_accounts_token() {
        let vault = MemoryVault::default();
        vault
            .save("one", &Secret::new("synthetic-a".to_owned()))
            .unwrap();
        vault
            .save("two", &Secret::new("synthetic-b".to_owned()))
            .unwrap();
        vault.remove("one").unwrap();
        assert!(vault.load("one").unwrap().is_none());
        assert_eq!(vault.load("two").unwrap().unwrap().expose(), "synthetic-b");
    }
}
