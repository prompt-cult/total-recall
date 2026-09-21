//! Shared helpers for integration tests that mutate process environment.
//! Env mutation is process-global and `unsafe` under edition 2024, so every
//! test that touches it must funnel through `lock_env()` to serialize.

use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serialize access to process-environment mutation across tests.
pub fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Restores an environment variable to its prior value on drop.
pub struct EnvGuard {
    key: &'static str,
    saved: Option<String>,
}

impl EnvGuard {
    pub fn set(key: &'static str, value: &str) -> Self {
        let saved = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self { key, saved }
    }

    pub fn unset(key: &'static str) -> Self {
        let saved = std::env::var(key).ok();
        unsafe { std::env::remove_var(key) };
        Self { key, saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.saved {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}
