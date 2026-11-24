//! Microsoft 会话、账户与游戏启动授权。

pub mod accounts;
pub mod microsoft;
pub mod skin;
pub mod tokens;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::redact::Secret;
use microsoft::VerifiedSession;

#[derive(Debug, thiserror::Error)]
pub enum LaunchAuthorizationError {
    #[error("本次正版游玩核验已失效，请重新启动以联网验证")]
    Expired,
    #[error("选中的账户已变更，请重新启动")]
    Revoked,
    #[error("已取消启动")]
    Cancelled,
}

/// 登录、切换或退出账号时撤销上一代的游戏启动授权。
#[derive(Debug, Default, Clone)]
pub struct LaunchAuthority(Arc<Mutex<u64>>);

impl LaunchAuthority {
    fn lock(&self) -> MutexGuard<'_, u64> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 在联网验证开始前读取。账户变化期间完成的验证不能用于启动。
    pub fn generation(&self) -> u64 {
        *self.lock()
    }

    pub fn revoke(&self) {
        let mut generation = self.lock();
        *generation = generation.wrapping_add(1);
    }

    /// 由刚通过官方核验的会话派生游戏身份。
    pub fn authorize(
        &self,
        generation: u64,
        session: VerifiedSession,
    ) -> Result<LaunchAccount, LaunchAuthorizationError> {
        check_session(generation, *self.lock(), session.valid_until())?;
        Ok(LaunchAccount {
            name: session.profile().name.clone(),
            uuid: session.profile().uuid,
            token: session.game_token().clone(),
            valid_until: session.valid_until(),
            authority: self.clone(),
            generation,
            used: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// 仅用于本次启动准备的身份。包含的令牌不参与序列化或日志。
#[derive(Debug, Clone)]
pub struct LaunchAccount {
    name: String,
    uuid: Uuid,
    token: Secret<String>,
    valid_until: Instant,
    authority: LaunchAuthority,
    generation: u64,
    used: Arc<AtomicBool>,
}

impl LaunchAccount {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn uuid(&self) -> Uuid {
        self.uuid
    }

    pub(crate) fn token(&self) -> &str {
        self.token.expose()
    }

    pub(crate) fn ensure_current(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), LaunchAuthorizationError> {
        self.check_current(cancel, *self.authority.lock())
    }

    /// 文件准备不能使用另一个账户。允许原授权在耗时准备期间到期，绑定参数前仍须重新验证。
    pub(crate) fn ensure_selected(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), LaunchAuthorizationError> {
        if cancel.is_cancelled() {
            return Err(LaunchAuthorizationError::Cancelled);
        }
        if *self.authority.lock() != self.generation {
            return Err(LaunchAuthorizationError::Revoked);
        }
        Ok(())
    }

    /// 启动进程和账户撤销按同一把锁排序；每份核验只允许一次创建进程的尝试。
    pub(crate) fn use_for_spawn<T>(
        &self,
        cancel: &CancellationToken,
        spawn: impl FnOnce() -> T,
    ) -> Result<T, LaunchAuthorizationError> {
        let current = self.authority.lock();
        self.check_current(cancel, *current)?;
        self.used.store(true, Ordering::Relaxed);
        let result = spawn();
        drop(current);
        Ok(result)
    }

    fn check_current(
        &self,
        cancel: &CancellationToken,
        generation: u64,
    ) -> Result<(), LaunchAuthorizationError> {
        if cancel.is_cancelled() {
            return Err(LaunchAuthorizationError::Cancelled);
        }
        check_session(self.generation, generation, self.valid_until)?;
        if self.used.load(Ordering::Relaxed) {
            return Err(LaunchAuthorizationError::Expired);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn for_test(authority: &LaunchAuthority, valid_until: Instant) -> Self {
        Self {
            name: "Steve".to_owned(),
            uuid: Uuid::from_u128(7),
            token: Secret::new("fixture-game-token".to_owned()),
            valid_until,
            authority: authority.clone(),
            generation: authority.generation(),
            used: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn check_session(
    issued_generation: u64,
    current_generation: u64,
    valid_until: Instant,
) -> Result<(), LaunchAuthorizationError> {
    if issued_generation != current_generation {
        return Err(LaunchAuthorizationError::Revoked);
    }
    if Instant::now() >= valid_until {
        return Err(LaunchAuthorizationError::Expired);
    }
    Ok(())
}
