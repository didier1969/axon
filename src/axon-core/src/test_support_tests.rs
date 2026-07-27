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
        "runtime_capacity_profile.rs",
        "embedder.rs",
        "indexer_health_http.rs",
        "vector_control.rs",
        "postgres/bulk_writer.rs",
        // REQ-AXO-902261 — REMOVED, each one fixed:
        //   `mcp/tests/mod.rs`  — its `env_lock()` minted a private mutex; now delegates
        //                         to `test_support::env_test_lock()`.
        //   `bin/axonctl.rs`    — could not reach the lock at all (`test_support` is
        //                         `#[cfg(test)]` on the LIB, and axonctl is a separate
        //                         binary crate). Fixed by removing the dependency on
        //                         process-global state instead: the env override is now a
        //                         parameter of a pure `indexer_health_port_from`, which
        //                         also gained coverage for the malformed-override case
        //                         nothing tested before.
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

/// REQ-AXO-902261 — the guard above only knows about ENV. The class is wider: **any
/// process-global resource serialized by a lock that is duplicated instead of shared**.
///
/// Found live, not hypothesized. `runtime_readiness::registry()` is one global singleton,
/// and THREE test files each declared their own private `registry_test_lock()` with its
/// own `static`. Three mutexes, one registry, zero exclusion across files. It surfaced as
/// `watchdog_observes_dead_heartbeat_after_threshold` failing with `got []` in a full-suite
/// run — 1682 passed, 1 failed — because a sibling file called `reset_for_tests()` inside
/// the 400 ms sleep window of the test that was about to assert. Load-dependent, rare, and
/// indistinguishable from a real watchdog regression.
///
/// What made it survive: the header of `runtime_watchdog_tests.rs` ASSERTED that these
/// tests "acquire a shared mutex with the runtime_readiness tests". They did not. Same
/// failure mode as the "DBQ-A claim feeder drains the backlog by construction" comment
/// (REQ-AXO-902260) — a confident sentence describing a mechanism that is not there tells
/// the reader the problem is already handled.
///
/// The rule enforced here: **only `test_support` may MINT a test lock.** A minting
/// definition is a `fn …_lock()` whose body creates its own `OnceLock` — that is what
/// produces a second mutex. A function that merely DELEGATES to `test_support` contains no
/// `OnceLock` and is fine; that is how `runtime_boot.rs` has always done it.
///
/// A first version of this guard only flagged the same `fn <name>_test_lock()` appearing
/// in two files. It passed — and missed a live offender found minutes later:
/// `mcp/tests/mod.rs` minted its own `env_lock()` (different NAME, same defect) while
/// `runtime_boot.rs` delegated, so the MCP tests serialized against nothing else in the
/// crate. Matching on names was matching on spelling; minting is the actual defect.
#[test]
fn no_test_lock_is_minted_outside_test_support() {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    // lock fn name -> files that MINT it (own OnceLock in the body)
    let mut definitions: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            // `test_support.rs` is the ONE place allowed to mint. Its own tests file is
            // this one, which mints nothing.
            if rel == "test_support.rs" {
                continue;
            }
            let lines: Vec<&str> = text.lines().collect();
            for (idx, raw) in lines.iter().enumerate() {
                let line = raw.trim_start();
                // A DEFINITION, not a call: `fn x_lock(` / `pub fn x_lock(` / …
                let Some(rest) = line
                    .strip_prefix("pub fn ")
                    .or_else(|| line.strip_prefix("pub(crate) fn "))
                    .or_else(|| line.strip_prefix("fn "))
                else {
                    continue;
                };
                let Some(name) = rest.split('(').next() else { continue };
                if !name.ends_with("_lock") {
                    continue;
                }
                // MINTING = the body creates its own lazily-initialized static. A
                // delegating wrapper has no `OnceLock` and is the correct pattern.
                //
                // The body is delimited by BRACE DEPTH, not by a line budget. A first
                // version scanned a fixed 12-line window and reported `mcp/tests/mod.rs`
                // as an offender AFTER it had been fixed: the window ran past the closing
                // brace into the next function, which legitimately holds a `OnceLock`. A
                // guard that accuses a corrected file is worse than no guard — it teaches
                // people to distrust it.
                let mut depth = 0i32;
                let mut mints = false;
                for body_line in &lines[idx..] {
                    if body_line.contains("OnceLock") && depth > 0 {
                        mints = true;
                    }
                    depth += body_line.matches('{').count() as i32;
                    depth -= body_line.matches('}').count() as i32;
                    if depth <= 0 && body_line.contains('}') {
                        break;
                    }
                }
                if !mints {
                    continue;
                }
                definitions
                    .entry(name.to_string())
                    .or_default()
                    .push(rel.clone());
            }
        }
    }

    let duplicated: Vec<String> = definitions
        .iter()
        .map(|(name, files)| format!("{name} minted in: {}", files.join(", ")))
        .collect();

    assert!(
        duplicated.is_empty(),
        "REQ-AXO-902261 — a test lock is MINTED outside `test_support`. Each of these \
         creates its OWN `OnceLock`, so it is a separate mutex: it serializes its own file \
         against itself and against NOTHING ELSE, while the resource it guards (process \
         env, the readiness registry, …) is global to the whole test binary:\n  {}\n\n\
         Fix: delete the local `static` and DELEGATE —\n  \
         fn my_lock() -> std::sync::MutexGuard<'static, ()> {{\n      \
         crate::test_support::env_test_lock().lock().unwrap_or_else(|p| p.into_inner())\n  \
         }}\n\n\
         If the resource is new, add ONE minting accessor to `test_support` next to \
         `env_test_lock` / `registry_test_lock` and delegate to that. One global resource, \
         one lock.",
        duplicated.join("\n  ")
    );
}
