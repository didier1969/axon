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

/// REQ-AXO-902261 — the GUARD. Fixing the known offenders one by one does not hold: the
/// next test added anywhere reintroduces the class. This walks the source and fails on a
/// test function that mutates process env without holding `env_test_lock()`.
///
/// Why it matters concretely (session 104): `inline_pipeline_enabled_for_truthy_values`
/// failed a full-suite run — `remove_var` from a sibling test landed between its
/// `set_var("true")` and its assertion. The Rust harness runs tests in parallel threads
/// within ONE process, so env is shared state.
///
/// And three files carried the comment "the suite pins --test-threads=1" as their
/// justification for NOT locking. **That claim is false**: there is no `.cargo/config.toml`
/// and nothing anywhere sets `--test-threads`. Those tests believed they were protected by
/// a guarantee that does not exist — the same shape as the "DBQ-A claim feeder drains the
/// backlog by construction" comment describing a feeder that is absent from the code
/// (REQ-AXO-902260). A comment asserting an invariant needs a test behind it or it
/// eventually lies. This is that test.
///
/// ALLOWLIST semantics: entries are files with KNOWN unlocked mutations, kept so the guard
/// can land before all 43 call sites are converted. Shrinking it is the work; a NEW file
/// cannot be added without someone reading this doc comment.
#[test]
fn no_test_mutates_process_env_without_the_lock() {
    use std::path::PathBuf;

    // Files whose test functions still mutate env unlocked (REQ-AXO-902261 inventory).
    // ONLY REMOVE entries — never add. Each removal is a fixed file.
    const KNOWN_UNLOCKED: &[&str] = &[
        "mcp/tests/context_and_analysis.rs",
        "mcp/tests/guidance_contract.rs",
        "mcp/tests/mod.rs",
        "runtime_capacity_profile.rs",
        "embedder.rs",
        "indexer_health_http.rs",
        "vector_control.rs",
        "postgres/bulk_writer.rs",
        "bin/axonctl.rs",
    ];
    const LOCK_MARKERS: &[&str] =
        &["env_test_lock", "ENV_LOCK", "EnvVarGuard", "registry_test_lock"];

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let mutates = text.contains("env::set_var") || text.contains("env::remove_var");
            if !mutates || !text.contains("#[test]") {
                continue;
            }
            if LOCK_MARKERS.iter().any(|m| text.contains(m)) {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if KNOWN_UNLOCKED.iter().any(|k| rel.ends_with(k)) {
                continue;
            }
            offenders.push(rel);
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "REQ-AXO-902261 — these files mutate process env in tests without holding \
         `test_support::env_test_lock()`, which makes the suite non-deterministic \
         (a sibling test's set_var/remove_var can land mid-assertion):\n  {}\n\n\
         Fix: take the lock first —\n  \
         let _env = crate::test_support::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());\n\n\
         Do NOT silence this by adding the file to KNOWN_UNLOCKED, and do NOT rely on \
         `--test-threads=1`: nothing in this repo sets it (verified — no .cargo/config.toml).",
        offenders.join("\n  ")
    );
}
