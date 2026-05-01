// REQ-AXO-099 Option B — sibling tests for the EnvVarGuard contract.
// Each test acquires the env_test_lock first so concurrent tests do
// not race on `std::env::set_var`. Each test uses a unique env var
// name to keep the post-Drop assertions deterministic when run
// alongside the rest of the suite.

use super::{env_test_lock, EnvVarGuard};

#[test]
fn env_var_guard_set_then_drop_restores_unset_state() {
    let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    const VAR: &str = "AXON_TEST_SUPPORT_SET_RESTORES_UNSET";
    // Establish prior=unset under the lock.
    std::env::remove_var(VAR);
    {
        let _guard = EnvVarGuard::set(VAR, "during_test");
        assert_eq!(std::env::var(VAR).ok(), Some("during_test".into()));
    }
    assert_eq!(
        std::env::var(VAR).ok(),
        None,
        "Drop must restore the unset prior state, not leave the test value"
    );
}

#[test]
fn env_var_guard_set_then_drop_restores_prior_value() {
    let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    const VAR: &str = "AXON_TEST_SUPPORT_SET_RESTORES_PRIOR";
    std::env::set_var(VAR, "prior_value");
    {
        let _guard = EnvVarGuard::set(VAR, "shadowed");
        assert_eq!(std::env::var(VAR).ok(), Some("shadowed".into()));
    }
    assert_eq!(
        std::env::var(VAR).ok(),
        Some("prior_value".into()),
        "Drop must restore the exact prior value"
    );
    std::env::remove_var(VAR);
}

#[test]
fn env_var_guard_unset_then_drop_restores_prior_value() {
    let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    const VAR: &str = "AXON_TEST_SUPPORT_UNSET_RESTORES_PRIOR";
    std::env::set_var(VAR, "to_restore");
    {
        let _guard = EnvVarGuard::unset(VAR);
        assert_eq!(std::env::var(VAR).ok(), None);
    }
    assert_eq!(
        std::env::var(VAR).ok(),
        Some("to_restore".into()),
        "unset guard must restore the prior set value"
    );
    std::env::remove_var(VAR);
}

#[test]
fn env_var_guard_survives_panic_in_test_body() {
    let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    const VAR: &str = "AXON_TEST_SUPPORT_SURVIVES_PANIC";
    std::env::set_var(VAR, "before_panic");
    let outcome = std::panic::catch_unwind(|| {
        let _guard = EnvVarGuard::set(VAR, "during_panic");
        panic!("simulated test failure");
    });
    assert!(outcome.is_err(), "the test body must have panicked");
    assert_eq!(
        std::env::var(VAR).ok(),
        Some("before_panic".into()),
        "Drop must restore prior value even when test body panics — this is the leak-prevention contract"
    );
    std::env::remove_var(VAR);
}

#[test]
fn multiple_env_var_guards_in_same_test_do_not_deadlock() {
    let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    const VAR_A: &str = "AXON_TEST_SUPPORT_MULTI_A";
    const VAR_B: &str = "AXON_TEST_SUPPORT_MULTI_B";
    std::env::remove_var(VAR_A);
    std::env::remove_var(VAR_B);
    {
        // Two guards in same test: caller holds the lock; the
        // guards do not re-acquire, so no deadlock.
        let _ga = EnvVarGuard::set(VAR_A, "a");
        let _gb = EnvVarGuard::set(VAR_B, "b");
        assert_eq!(std::env::var(VAR_A).ok(), Some("a".into()));
        assert_eq!(std::env::var(VAR_B).ok(), Some("b".into()));
    }
    assert_eq!(std::env::var(VAR_A).ok(), None);
    assert_eq!(std::env::var(VAR_B).ok(), None);
}
