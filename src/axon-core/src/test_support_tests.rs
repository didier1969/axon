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

// ---------------------------------------------------------------------------
// REQ-AXO-902261 — source scanners shared by the two guards below.
// ---------------------------------------------------------------------------

/// Every `.rs` file under `src/`, as `(path relative to src/, contents)`.
fn crate_sources() -> Vec<(String, String)> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
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
            out.push((rel, text));
        }
    }
    out.sort();
    out
}

/// Index of the line closing the block opened at or after `start`, by brace depth.
///
/// A line budget was tried here first and was wrong: an earlier version of the minting
/// guard scanned a fixed 12-line window from the `fn`, ran past the closing brace into
/// the next function, and reported `mcp/tests/mod.rs` as an offender AFTER it had been
/// fixed. A guard that accuses a corrected file teaches people to distrust it.
fn block_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        depth += line.matches('{').count() as i32;
        if line.contains('{') {
            opened = true;
        }
        depth -= line.matches('}').count() as i32;
        if opened && depth <= 0 {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

fn mutates_env(text: &str) -> bool {
    text.contains("env::set_var") || text.contains("env::remove_var")
}

/// `Mutex<()>` — a serialization lock. `Mutex<Vec<_>>` / `Mutex<HashMap<_>>` is a pool or
/// a registry protecting a collection, which is an ordinary design choice and must not be
/// reported: `mcp/tests/mod.rs` parks test databases that way.
fn is_unit_mutex(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("Mutex<()>")
}

fn is_test_fn(lines: &[&str], start: usize) -> bool {
    lines[start.saturating_sub(6)..start]
        .iter()
        .any(|l| l.contains("#[test]") || l.contains("#[tokio::test"))
}

fn fn_name_at(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("pub(crate) fn ")
        .or_else(|| trimmed.strip_prefix("pub async fn "))
        .or_else(|| trimmed.strip_prefix("pub fn "))
        .or_else(|| trimmed.strip_prefix("async fn "))
        .or_else(|| trimmed.strip_prefix("fn "))?;
    let name = rest
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("");
    (!name.is_empty()).then_some(name)
}

fn static_name_at(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("pub static ")
        .or_else(|| trimmed.strip_prefix("static "))?;
    let name = rest.split(':').next().unwrap_or("").trim();
    (!name.is_empty()).then_some(name)
}

/// `impl Foo {` / `impl Drop for Foo {` / `impl<T> Foo<T> {` → `Foo`.
fn impl_type_at(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("impl")?;
    let rest = rest.trim_start();
    let rest = if let Some(stripped) = rest.strip_prefix('<') {
        let close = stripped.find('>')?;
        &stripped[close + 1..]
    } else {
        rest
    };
    let tail = rest.rsplit(" for ").next().unwrap_or(rest);
    let ty: String = tail
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!ty.is_empty()).then_some(ty)
}

/// `(name, start, end, is_inside_an_impl)` for every `fn` in the file.
fn fn_blocks(lines: &[&str]) -> Vec<(String, usize, usize, bool)> {
    let impl_spans: Vec<(usize, usize)> = (0..lines.len())
        .filter(|i| impl_type_at(lines[*i]).is_some())
        .map(|i| (i, block_end(lines, i)))
        .collect();
    (0..lines.len())
        .filter_map(|i| fn_name_at(lines[i]).map(|n| (n.to_string(), i)))
        .map(|(name, i)| {
            let inside = impl_spans.iter().any(|(s, e)| *s < i && i <= *e);
            (name, i, block_end(lines, i), inside)
        })
        .collect()
}

fn impl_blocks(lines: &[&str]) -> Vec<(String, usize, usize)> {
    (0..lines.len())
        .filter_map(|i| impl_type_at(lines[i]).map(|t| (t, i, block_end(lines, i))))
        .collect()
}

/// A file whose WHOLE contents are test code — included by `mod tests;` or `#[path]`, so
/// it carries no `#[cfg(test)] mod` of its own.
///
/// This is the scope hole that let the largest offender in the repo hide: the minting
/// guard only walked inline `#[cfg(test)] mod … {` blocks, so `embedder/tests.rs` — 30
/// env-mutating test functions serialized by a private `static ENV_TEST_GUARD` — was
/// never even read.
fn is_test_file(rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base == "tests.rs" || base.ends_with("_tests.rs") || rel.contains("tests/")
}

fn test_regions(lines: &[&str], whole_file: bool) -> Vec<(usize, usize)> {
    if lines.is_empty() {
        return Vec::new();
    }
    if whole_file {
        return vec![(0, lines.len() - 1)];
    }
    let mut out = Vec::new();
    let mut pending_cfg_test = false;
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim_start();
        if line.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            continue;
        }
        if pending_cfg_test && line.starts_with("mod ") {
            if line.contains('{') {
                out.push((i, block_end(lines, i)));
            }
            pending_cfg_test = false;
        }
    }
    out
}

/// REQ-AXO-902261 — the ENV guard, per FUNCTION.
///
/// Why it matters concretely (session 104): `inline_pipeline_enabled_for_truthy_values`
/// failed a full-suite run — `remove_var` from a sibling test landed between its
/// `set_var("true")` and its assertion. The Rust harness runs tests in parallel threads
/// within ONE process, so env is shared state.
///
/// The first version of this guard matched lock markers per FILE against an allowlist,
/// and both halves of that design were wrong:
///
/// 1. **Per-file granularity.** `embedder/tests.rs` has ONE function that uses
///    `env_test_lock` (out of 31 that touch env) — enough for the whole file to be
///    exempted. Its other 30 functions, 180 mutation sites, were serialized by a private
///    `static ENV_TEST_GUARD` whose own comment admitted it "only serializes … within
///    this mod". The single largest env consumer in the crate read as compliant.
/// 2. **`EnvVarGuard` counted as a lock marker.** It is not one — `test_support`'s own
///    doc says the guard "does NOT acquire any lock itself" and lists holding the lock as
///    its PRECONDITION. Half the two-step contract was being accepted as the whole of it.
///    `registry_test_lock` was in the list too, and that lock guards a different resource.
///
/// So the check is now per function, and conformance is *derived* rather than spelled:
/// a test body must mention something whose call chain reaches `test_support::env_test_lock`.
/// Local delegating helpers (`env_lock`, `env_guard`, `lock_env_guard`) and RAII guard
/// types (`EnvGuard`, `RuntimeEnvGuard`, `SollSiteRootGuard`) are picked up automatically,
/// which is why no allowlist survives here: there is nothing left to exempt.
#[test]
fn no_test_mutates_process_env_without_the_lock() {
    use std::collections::BTreeSet;

    let sources = crate_sources();
    let mut conforming: BTreeSet<String> = BTreeSet::new();
    conforming.insert("env_test_lock".to_string());

    // TWO rounds, deliberately bounded: helper → RAII type → test body is the deepest
    // real chain (`EnvGuard::new` → `env_lock` → `env_test_lock`), and a third round adds
    // no new type. An UNBOUNDED fixpoint was measured first and was the wrong answer:
    // substring propagation reached 3180 identifiers — nearly every type in the crate —
    // and the guard went green on everything. A guard that cannot fail is not a guard.
    for _ in 0..2 {
        for (_, text) in &sources {
            let lines: Vec<&str> = text.lines().collect();
            for (name, start, end, inside_impl) in fn_blocks(&lines) {
                if inside_impl || conforming.contains(&name) {
                    continue;
                }
                let body = lines[start..=end].join("\n");
                if conforming.iter().any(|c| body.contains(c)) {
                    conforming.insert(name);
                }
            }
            for (ty, start, end) in impl_blocks(&lines) {
                // ONLY `*Guard` RAII types propagate. Without that restriction `impl
                // GraphStore` acquired conformance and any test merely NAMING GraphStore
                // read as serialized. `EnvVarGuard` is excluded by name for the reason in
                // the doc comment above: it takes no lock.
                if !ty.ends_with("Guard") || ty == "EnvVarGuard" || conforming.contains(&ty) {
                    continue;
                }
                let body = lines[start..=end].join("\n");
                if conforming.iter().any(|c| body.contains(c)) {
                    conforming.insert(ty);
                }
            }
        }
    }

    let mut offenders: Vec<String> = Vec::new();
    for (rel, text) in &sources {
        if rel == "test_support.rs" || !mutates_env(text) {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (name, start, end, _) in fn_blocks(&lines) {
            if !is_test_fn(&lines, start) {
                continue;
            }
            let body = lines[start..=end].join("\n");
            if !mutates_env(&body) || conforming.iter().any(|c| body.contains(c)) {
                continue;
            }
            offenders.push(format!("{rel}:{}  {name}", start + 1));
        }
    }
    offenders.sort();

    assert!(
        offenders.is_empty(),
        "REQ-AXO-902261 — these TEST FUNCTIONS mutate the process environment without \
         holding a lock that reaches `test_support::env_test_lock()`, which makes the \
         suite non-deterministic (a sibling test's set_var/remove_var can land \
         mid-assertion):\n  {}\n\n\
         Fix: take the lock as the FIRST statement —\n  \
         let _env = crate::test_support::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());\n\n\
         An `EnvVarGuard` alone does NOT satisfy this: it restores prior values, it does \
         not serialize (see its doc — holding the lock is its PRECONDITION). Delegating \
         through a local helper is fine and is detected automatically. Do NOT rely on \
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
/// `watchdog_observes_dead_heartbeat_after_threshold` failing with `got []` in a
/// full-suite run — 1682 passed, 1 failed — because a sibling file called
/// `reset_for_tests()` inside the 400 ms sleep window of the test about to assert.
///
/// What made it survive: the header of `runtime_watchdog_tests.rs` ASSERTED that these
/// tests "acquire a shared mutex with the runtime_readiness tests". They did not. Same
/// failure mode as the "DBQ-A claim feeder drains the backlog by construction" comment
/// (REQ-AXO-902260) — a confident sentence describing a mechanism that is not there tells
/// the reader the problem is already handled.
///
/// The rule enforced: **only `test_support` may MINT a test lock.** Minting is a
/// `Mutex<()>` brought into being outside it — as a `static`, or lazily inside a `fn` via
/// `OnceLock`. A function that merely DELEGATES contains neither and is correct.
///
/// Three spellings of the same defect have now been missed by three successive versions
/// of this check, which is the argument for matching on SHAPE rather than on names:
///
/// | Spelling | Missed because |
/// |---|---|
/// | `fn registry_test_lock()` × 3 files | first version compared names across files only |
/// | `mcp/tests/mod.rs::env_lock()` | different NAME, same defect — matching on spelling |
/// | `embedder/tests.rs::ENV_TEST_GUARD` | suffix `_GUARD` not `_LOCK`, **and** the file has no inline `#[cfg(test)] mod` so it was out of scope entirely |
///
/// Widening the scope to whole test FILES immediately surfaced a fourth: `service_guard.rs`
/// held BOTH a public `lock_for_tests()` and a private `static TEST_GUARD` over the same
/// global atomics, with its own 24 tests taking the private one while `embedder/tests.rs`
/// took the public one — while `lock_for_tests`'s doc said every test touching that state
/// must hold IT.
#[test]
fn no_test_lock_is_minted_outside_test_support() {
    let mut offenders: Vec<String> = Vec::new();
    for (rel, text) in crate_sources() {
        // `test_support.rs` is the ONE place allowed to mint, and this file is its test
        // sibling: its assertion strings quote both `OnceLock` and `Mutex<()>`.
        if rel == "test_support.rs" || rel == "test_support_tests.rs" {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (region_start, region_end) in test_regions(&lines, is_test_file(&rel)) {
            for i in region_start..=region_end {
                if let Some(name) = static_name_at(lines[i]) {
                    if is_unit_mutex(lines[i]) {
                        offenders.push(format!("{rel}:{}  static {name}", i + 1));
                        continue;
                    }
                }
                if let Some(name) = fn_name_at(lines[i]) {
                    let body = lines[i..=block_end(&lines, i)].join("\n");
                    if body.contains("OnceLock") && is_unit_mutex(&body) {
                        offenders.push(format!("{rel}:{}  fn {name}()", i + 1));
                    }
                }
            }
        }
    }
    offenders.sort();
    offenders.dedup();

    assert!(
        offenders.is_empty(),
        "REQ-AXO-902261 — a test lock is MINTED outside `test_support`. Each of these \
         creates its OWN `Mutex<()>`, so it is a separate mutex: it serializes its own \
         file against itself and against NOTHING ELSE, while the resource it guards \
         (process env, the readiness registry, the service_guard atomics, …) is global to \
         the whole test binary:\n  {}\n\n\
         Fix: delete the local lock and DELEGATE —\n  \
         fn my_lock() -> std::sync::MutexGuard<'static, ()> {{\n      \
         crate::test_support::env_test_lock().lock().unwrap_or_else(|p| p.into_inner())\n  \
         }}\n\n\
         If the resource is genuinely new, add ONE minting accessor to `test_support` \
         next to `env_test_lock` / `registry_test_lock` / `service_guard_test_lock` and \
         delegate to that. One global resource, one lock.",
        offenders.join("\n  ")
    );
}

/// REQ-AXO-902299 — no error hint may prescribe raw SQL when a tool answers the need.
///
/// REQ-AXO-902246 established that the `sql` traffic came from our OWN procedures
/// prescribing raw SQL, and shipped `soll_get` / `soll_children` to replace it. The
/// procedures were repointed; the PRODUCT's error messages were not. Nine hints
/// still sent the caller to the SQL console, and the surface contradicted itself:
/// the `soll_get` catalog entry says "use this INSTEAD OF
/// `sql SELECT description FROM soll.Node WHERE id=…`" while another hint
/// prescribed that exact query. One of them could not even work — it filtered on
/// `source_id = '<replacement>'`, the very value the caller was looking for.
///
/// Scoped to hint-ish fields so a legitimate mention ("use X instead of sql …" in a
/// tool description) is not caught. The allow-list carries a REASON, not just a
/// name: `soll.Revision` has no read tool, so pointing at `sql` there is a
/// deliberate answer rather than an oversight.
#[test]
fn no_error_hint_prescribes_raw_sql_when_a_tool_exists() {
    // (file fragment, why it may keep prescribing SQL)
    const ALLOWED: &[(&str, &str)] = &[(
        "planning_revision.rs",
        "soll.Revision has no dedicated read tool; the SQL pointer is the answer",
    )];

    let mut offenders: Vec<String> = Vec::new();
    for (path, text) in crate_sources() {
        // `crate_sources` yields paths RELATIVE to `src/` ("mcp/tools_soll/x.rs"),
        // so a leading-slash filter matches nothing and the guard silently scans
        // zero files. Caught by injecting a canary — which is the only way this
        // class of mistake ever surfaces.
        if !path.starts_with("mcp/") {
            continue;
        }
        if ALLOWED.iter().any(|(frag, _)| path.contains(frag)) {
            continue;
        }
        for (idx, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            let is_hint = lower.contains("hint")
                || lower.contains("next_action")
                || lower.contains("recovery_hint");
            if is_hint && lower.contains("sql select") {
                offenders.push(format!("{}:{} — {}", path, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these hints send an LLM to the raw SQL console instead of the tool that \
         answers the need (REQ-AXO-902299). Point at `soll_get` (one node body), \
         `soll_children` (edges), `soll_query_context` (project picture) or `query` \
         (IST ids). If a need genuinely has no tool, add it to ALLOWED with the \
         reason:\n  {}",
        offenders.join("\n  ")
    );
}

/// REQ-AXO-902274 — no test may touch the PROCESS-GLOBAL service state without
/// holding the shared lock.
///
/// `service_guard`'s atomics and `UtilityFirstScheduler` are process-global. A
/// test that resets or records into them while another test reads them corrupts
/// that other test, not itself — which is why the symptom always appeared far
/// from the cause: `semantic_policy` returned `gpu_cadence_refill` instead of
/// `balanced_drain`, green in isolation, red in parallel, on code nobody had
/// touched. Diagnosing it cost a full build gate twice in one session.
///
/// The sibling guard `no_test_mutates_process_env_without_the_lock` covers the
/// ENV. This one covers the service state, and both share the same reasoning: one
/// global resource ⇒ one lock, crate-wide.
///
/// Conformance is derived, not spelled: any body mentioning something whose call
/// chain reaches the lock counts (`lock_for_tests`, `lock_service_guard`,
/// `service_guard_test_lock`). Order matters elsewhere — every conforming site
/// takes `env_lock` FIRST, then this one; keeping that order uniform is what
/// prevents a deadlock.
#[test]
fn no_test_touches_global_service_state_without_the_lock() {
    const TOUCHES: &[&str] = &[
        "service_guard::reset_for_tests",
        "service_guard::record_",
        "reset_utility_first_scheduler_for_tests",
    ];
    const CONFORMING: &[&str] = &[
        "lock_for_tests",
        "lock_service_guard",
        "service_guard_test_lock",
        "_sg_guard",
    ];

    let mut offenders: Vec<String> = Vec::new();
    for (path, text) in crate_sources() {
        // Production code legitimately drives this state; only TESTS must queue.
        if !path.contains("tests") {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (name, start, end, inside_impl) in fn_blocks(&lines) {
            if inside_impl {
                continue;
            }
            let body = lines[start..=end].join("\n");
            if !TOUCHES.iter().any(|t| body.contains(t)) {
                continue;
            }
            if CONFORMING.iter().any(|c| body.contains(c)) {
                continue;
            }
            offenders.push(format!("{path}::{name}"));
        }
    }

    assert!(
        offenders.is_empty(),
        "these tests mutate or read PROCESS-GLOBAL service state without holding \
         `service_guard::lock_for_tests()` (REQ-AXO-902274). They corrupt OTHER \
         tests, so the failure surfaces far from here and looks like a real \
         regression. Take `env_lock()` first, then the service-guard lock — the \
         order is uniform across the crate and that is what avoids a \
         deadlock:\n  {}",
        offenders.join("\n  ")
    );
}

/// REQ-AXO-902414 — l'instantane de reglage runtime est un cache PROCESSUS
/// (`RUNTIME_TUNING_SNAPSHOT`, un `OnceLock<Mutex<Option<..>>>`) rempli par
/// `get_or_insert` : le premier test du processus qui le touche fixe la valeur
/// pour tous les suivants, et le `bootstrap` recalcule depuis l'environnement
/// est alors CALCULE puis JETE, sans un mot.
///
/// Consequence vecue : `test_single_gpu_worker_cruise_mode_grows_more_...`
/// posait `AXON_VECTOR_WORKERS=1`, lisait 8 dans l'instantane, et rendait 80 au
/// lieu de 104. Vert en isolation, rouge en suite, et le simple ajout de trois
/// tests ailleurs dans le crate suffisait a faire basculer le verdict.
///
/// PORTEE DE CETTE GARDE, dite franchement : elle balaie les tests qui POSENT
/// une des variables du bootstrap. Elle ne detecte PAS un test qui lirait
/// l'instantane sans poser aucune variable — celui-la herite aussi, mais rien
/// dans sa source ne le trahit statiquement.
#[test]
fn no_test_sets_a_runtime_tuning_env_var_without_establishing_the_snapshot() {
    const ESTABLISHES: &[&str] = &[
        "refresh_runtime_tuning_snapshot_from_env",
        "reset_runtime_tuning_snapshot",
    ];
    const MARKER: &str = "RUNTIME-TUNING-SNAPSHOT-OK:";
    // Les deux fonctions dont la source DEFINIT l'etat memoise.
    const BOOTSTRAPS: &[&str] = &[
        "bootstrap_runtime_tuning_state_from_env",
        "bootstrap_embedding_lane_config_from_env",
    ];

    let sources = crate_sources();

    // --- L'entree de la garde est DERIVEE, pas recopiee -----------------------
    // On ne retient que les litteraux lus DANS le corps du bootstrap : ce sont
    // exactement les variables dont la valeur n'atteint le code teste QUE par
    // l'instantane. Un saut d'indirection de plus happerait
    // `AXON_EMBEDDING_PROVIDER`, qui dispose aussi d'une voie de lecture directe
    // (`embedding_provider_requested_is_gpu`) : le poser n'est donc pas en soi
    // une dependance a l'ordre, et la garde crierait sur ~17 tests sains.
    let collect_vars = |body: &str, out: &mut Vec<String>| {
        let mut rest = body;
        while let Some(at) = rest.find("\"AXON_") {
            rest = &rest[at + 1..];
            if let Some(close) = rest.find('"') {
                let var = &rest[..close];
                if var
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                    && !out.iter().any(|v| v == var)
                {
                    out.push(var.to_string());
                }
            }
        }
    };

    let mut seed_bodies: Vec<String> = Vec::new();
    for (_, text) in &sources {
        let lines: Vec<&str> = text.lines().collect();
        for (name, start, end, _) in fn_blocks(&lines) {
            if BOOTSTRAPS.contains(&name.as_str()) {
                seed_bodies.push(lines[start..=end].join("\n"));
            }
        }
    }

    let mut tuning_vars: Vec<String> = Vec::new();
    for body in &seed_bodies {
        collect_vars(body, &mut tuning_vars);
    }
    tuning_vars.sort();

    // Un denominateur vide rendrait cette garde verte pour de mauvaises raisons.
    assert!(
        tuning_vars.len() >= 10,
        "la derivation n'a trouve que {} variable(s) dans {BOOTSTRAPS:?} : \
         les fonctions ont ete renommees ou deplacees, et cette garde ne \
         surveille donc plus rien. Reaccorde `BOOTSTRAPS` avant de croire un \
         resultat vert.\n  trouvees : {:?}",
        tuning_vars.len(),
        tuning_vars
    );

    // --- Balayage -------------------------------------------------------------
    let mut scanned = 0usize;
    let mut setters = 0usize;
    let mut offenders: Vec<String> = Vec::new();
    for (path, text) in &sources {
        if !path.contains("test") {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (name, start, end, inside_impl) in fn_blocks(&lines) {
            if inside_impl {
                continue;
            }
            let body = lines[start..=end].join("\n");
            scanned += 1;
            let touched: Vec<&String> = tuning_vars
                .iter()
                .filter(|v| body.contains(&format!("set_var(\"{v}\"")))
                .collect();
            if touched.is_empty() {
                continue;
            }
            setters += 1;
            if body.contains(MARKER) {
                continue;
            }
            // L'instantane doit etre etabli APRES les `set_var` et AVANT que le
            // test n'exerce quoi que ce soit — un rafraichissement place en
            // seule teardown satisfait un `contains` naif tout en laissant le
            // test dependre de l'ordre. C'est la faiblesse que la falsification
            // de cette garde a mise au jour (REQ-AXO-902414).
            // Borne basse : le DERNIER `set_var` surveille — etablir avant lui
            // ne sert a rien, la variable suivante n'est pas encore posee.
            let last_set = touched
                .iter()
                .filter_map(|v| body.rfind(&format!("set_var(\"{v}\"")))
                .max()
                .unwrap_or(0);
            // Borne haute : le DERNIER `remove_var` surveille. Un `remove_var`
            // isole ne marque pas la teardown — les setups en contiennent aussi
            // (remise a zero des drapeaux `_AUTOCONFIGURED`), ce qui coupait la
            // fenetre trop tot et accusait des tests sains.
            let teardown_at = touched
                .iter()
                .filter_map(|v| body.rfind(&format!("remove_var(\"{v}\"")))
                .max()
                .unwrap_or(body.len());
            let established_in_body = ESTABLISHES
                .iter()
                .any(|e| body.match_indices(e).any(|(at, _)| at > last_set && at < teardown_at));
            if established_in_body {
                continue;
            }
            offenders.push(format!(
                "{path}::{name}  [{}]",
                touched
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "ces tests posent une variable du bootstrap de reglage runtime puis \
         laissent l'instantane MEMOISE decider a leur place (REQ-AXO-902414).\n\
         Mecanisme : `current_runtime_tuning_snapshot` fait `get_or_insert` — si \
         un test anterieur a rempli l'emplacement, ton `set_var` est decoratif et \
         ton assertion mesure l'ordre d'execution, pas le comportement. La \
         suite rougit alors LOIN d'ici et ressemble a une vraie regression.\n\
         Geste : apres tes `set_var` et AVANT d'exercer quoi que ce soit, appelle \
         `super::refresh_runtime_tuning_snapshot_from_env()` — un appel place \
         en seule teardown ne compte pas, il n'etablit rien pour TON test. \
         Rappelle-le aussi apres tes `remove_var`, sinon tu repasses le \
         probleme au voisin.\n\
         Si ton test ne LIT pas l'instantane, dis-le sur place avec un \
         commentaire `// {MARKER} <raison>` : la raison est l'audit.\n\
         Denominateur : {scanned} fonctions balayees, {setters} posent une de ces \
         {} variables.\n  {}",
        tuning_vars.len(),
        offenders.join("\n  ")
    );
}
