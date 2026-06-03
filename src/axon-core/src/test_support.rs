//! REQ-AXO-099 Option B — panic-safe test fixtures for the
//! process-wide config bus (see project memory
//! `project_env_var_config_bus.md`).
//!
//! The contract is two-step:
//!
//! 1. **Acquire `env_test_lock()`** at the top of every test that
//!    touches process-wide env vars. The lock serializes ALL such
//!    tests across the whole crate, preventing the cross-test
//!    leakage that REQ-AXO-099 documents. Use the same `_lock`
//!    binding as the existing `runtime_readiness_tests` pattern.
//!
//! 2. **Mutate env vars only via `EnvVarGuard`**. Each guard saves
//!    the prior value on construction and restores it on Drop —
//!    including when the test body panics. Multiple guards in the
//!    same test are safe because the lock is held by the caller,
//!    not by the guard (so they do not deadlock each other).
//!
//! ```ignore
//! #[test]
//! fn my_test() {
//!     let _lock = test_support::env_test_lock()
//!         .lock()
//!         .unwrap_or_else(|p| p.into_inner());
//!     let _g1 = EnvVarGuard::set("AXON_OPT_MAX_VRAM_USED_MB", "9999");
//!     let _g2 = EnvVarGuard::unset("AXON_OPT_ALLOWED_ACTUATORS");
//!     // ... test body. _lock and the guards drop in reverse order
//!     // at scope end, restoring prior env state under the lock.
//! }
//! ```
//!
//! Why two steps and not one self-locking guard: nested guards on
//! the same thread would deadlock `std::sync::Mutex`. A reentrant
//! mutex would fix that but adds a non-trivial dependency. The
//! two-step pattern is what `runtime_readiness_tests` already
//! uses, and matches the project convention.

use std::sync::{Mutex, OnceLock};

/// Process-wide env-var serialization mutex. Every test that
/// mutates process env vars must acquire this lock first. The
/// mutex protects ALL env-var operations as a single unit because
/// env vars share one OS-level table; per-key serialization would
/// not protect cross-key invariants nor avoid the
/// `std::env::set_var` thread-safety hazard the Rust std documents.
pub fn env_test_lock() -> &'static Mutex<()> {
    static MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

/// RAII guard for one env-var mutation. The guard does NOT acquire
/// any lock itself — the caller holds `env_test_lock()` for the
/// guard's lifetime. Drop restores the prior value (or unsets if
/// the prior state was unset) and is panic-safe.
pub struct EnvVarGuard {
    name: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    /// Set `name` to `value`. Returns a guard that restores the
    /// prior value (or unsets) on Drop.
    ///
    /// PRECONDITION: caller holds `env_test_lock()`.
    pub fn set(name: &'static str, value: &str) -> Self {
        let prior = std::env::var(name).ok();
        std::env::set_var(name, value);
        Self { name, prior }
    }

    /// Remove `name`. Returns a guard that restores the prior
    /// value (or leaves unset if the prior state was unset) on
    /// Drop.
    ///
    /// PRECONDITION: caller holds `env_test_lock()`.
    pub fn unset(name: &'static str) -> Self {
        let prior = std::env::var(name).ok();
        std::env::remove_var(name);
        Self { name, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.prior.take() {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

#[cfg(test)]
#[path = "test_support_tests.rs"]
mod test_support_tests;

#[cfg(test)]
pub mod test_db;

#[cfg(test)]
pub mod ist_fixtures;
