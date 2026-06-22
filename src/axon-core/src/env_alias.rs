//! Env-var alias consolidation helpers (REQ-AXO-901657 slice 4).
//!
//! Non-breaking deprecation of redundant env-var aliases identified in the
//! 2026-05-22 env-vars audit (`docs/audits/2026-05-22-env-vars-inventory.md`).
//!
//! Contract :
//! - Canonical name is checked first.
//! - If the canonical is unset but the alias is set, the alias value is
//!   honored AND a `WARN deprecated_alias=<alias> canonical=<canonical>` log
//!   line is emitted **once per alias** (process-global).
//! - If both are set, canonical wins ; the alias is also flagged as
//!   deprecated so callers migrate.
//!
//! See SOLL `CPT-AXO-90026` for the 30-var canonical env-var set and
//! `REQ-AXO-901657` slice 4 for the consolidation plan.

use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::sync::Mutex;

/// Process-global set of alias names that have already emitted their
/// one-shot deprecation warning. Prevents log spam when an alias is read
/// from a hot loop (e.g. pipeline status snapshots).
static WARNED: Lazy<Mutex<HashSet<&'static str>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn warn_once(alias: &'static str, canonical: &'static str) {
    let mut guard = match WARNED.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.insert(alias) {
        tracing::warn!(
            deprecated_alias = alias,
            canonical = canonical,
            "deprecated env alias: use {canonical} instead of {alias} (REQ-AXO-901657)"
        );
    }
}

/// Read an env var that has a canonical name plus a deprecated alias.
///
/// Returns the first non-empty value found, checking canonical first. When
/// the alias provides the value (i.e. canonical unset), a deprecation
/// warning is logged once per process.
///
/// Empty strings are treated as unset (matches the rest of the runtime,
/// which trims+parses).
pub fn read_with_alias(canonical: &'static str, alias: &'static str) -> Option<String> {
    if let Ok(value) = std::env::var(canonical) {
        if !value.trim().is_empty() {
            // If alias is ALSO set, surface the duplication once so the
            // operator knows which one is winning.
            if std::env::var(alias)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
            {
                warn_once(alias, canonical);
            }
            return Some(value);
        }
    }
    if let Ok(value) = std::env::var(alias) {
        if !value.trim().is_empty() {
            warn_once(alias, canonical);
            return Some(value);
        }
    }
    None
}

/// Read an env var with canonical + alias and a default string fallback.
pub fn read_with_alias_or<S: Into<String>>(
    canonical: &'static str,
    alias: &'static str,
    default: S,
) -> String {
    read_with_alias(canonical, alias).unwrap_or_else(|| default.into())
}

/// Probe whether either canonical or alias is set (for diagnostics).
/// Emits the deprecation warning if only the alias is set.
pub fn alias_is_set(canonical: &'static str, alias: &'static str) -> bool {
    let canonical_set = std::env::var(canonical)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if canonical_set {
        return true;
    }
    let alias_set = std::env::var(alias)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if alias_set {
        warn_once(alias, canonical);
    }
    alias_set
}

/// Test helper : clear the warned set so a test can re-trigger the
/// one-shot warning deterministically.
#[cfg(test)]
pub fn __reset_warned_for_tests() {
    if let Ok(mut g) = WARNED.lock() {
        g.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // std::env is process-global ; serialize alias tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn canonical_wins_over_alias() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_TEST_CANON_A", "canonical");
            std::env::set_var("AXON_TEST_ALIAS_A", "alias");
        }
        let val = read_with_alias("AXON_TEST_CANON_A", "AXON_TEST_ALIAS_A");
        assert_eq!(val.as_deref(), Some("canonical"));
        unsafe {
            std::env::remove_var("AXON_TEST_CANON_A");
            std::env::remove_var("AXON_TEST_ALIAS_A");
        }
    }

    #[test]
    fn alias_used_when_canonical_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_TEST_CANON_B");
            std::env::set_var("AXON_TEST_ALIAS_B", "from-alias");
        }
        __reset_warned_for_tests();
        let val = read_with_alias("AXON_TEST_CANON_B", "AXON_TEST_ALIAS_B");
        assert_eq!(val.as_deref(), Some("from-alias"));
        unsafe {
            std::env::remove_var("AXON_TEST_ALIAS_B");
        }
    }

    #[test]
    fn both_unset_returns_none() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_TEST_CANON_C");
            std::env::remove_var("AXON_TEST_ALIAS_C");
        }
        let val = read_with_alias("AXON_TEST_CANON_C", "AXON_TEST_ALIAS_C");
        assert_eq!(val, None);
    }

    #[test]
    fn empty_string_treated_as_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_TEST_CANON_D", "");
            std::env::set_var("AXON_TEST_ALIAS_D", "alias-value");
        }
        __reset_warned_for_tests();
        let val = read_with_alias("AXON_TEST_CANON_D", "AXON_TEST_ALIAS_D");
        assert_eq!(val.as_deref(), Some("alias-value"));
        unsafe {
            std::env::remove_var("AXON_TEST_CANON_D");
            std::env::remove_var("AXON_TEST_ALIAS_D");
        }
    }
}
