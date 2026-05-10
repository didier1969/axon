// REQ-AXO-262 / GUI-PRO-001 — sibling tests for gpu_backend helpers.

use super::{ort_bind_output_per_iter_from_env, ort_memory_pattern_enabled_from_env};

#[test]
fn ort_memory_pattern_default_is_true() {
    assert!(ort_memory_pattern_enabled_from_env(None));
}

#[test]
fn ort_memory_pattern_zero_disables() {
    assert!(!ort_memory_pattern_enabled_from_env(Some("0")));
}

#[test]
fn ort_memory_pattern_false_disables_case_insensitive() {
    assert!(!ort_memory_pattern_enabled_from_env(Some("false")));
    assert!(!ort_memory_pattern_enabled_from_env(Some("False")));
    assert!(!ort_memory_pattern_enabled_from_env(Some("FALSE")));
}

#[test]
fn ort_memory_pattern_other_values_enable() {
    assert!(ort_memory_pattern_enabled_from_env(Some("1")));
    assert!(ort_memory_pattern_enabled_from_env(Some("true")));
    assert!(ort_memory_pattern_enabled_from_env(Some("on")));
    assert!(ort_memory_pattern_enabled_from_env(Some("yes")));
}

#[test]
fn ort_memory_pattern_trims_whitespace() {
    assert!(!ort_memory_pattern_enabled_from_env(Some(" 0 ")));
    assert!(!ort_memory_pattern_enabled_from_env(Some(" false ")));
}

#[test]
fn ort_memory_pattern_empty_string_enables() {
    // Defensive: an empty value is not the explicit-disable marker, so
    // the safe interpretation is "enabled" (matches the default).
    assert!(ort_memory_pattern_enabled_from_env(Some("")));
}

// REQ-AXO-262 / VAL-AXO-055 — bind_output_per_iter parser tests.
// Default flipped to TRUE after empirical regression (see VAL-AXO-055).

#[test]
fn ort_bind_output_per_iter_default_is_true() {
    assert!(ort_bind_output_per_iter_from_env(None));
}

#[test]
fn ort_bind_output_per_iter_zero_disables() {
    assert!(!ort_bind_output_per_iter_from_env(Some("0")));
}

#[test]
fn ort_bind_output_per_iter_false_case_insensitive() {
    assert!(!ort_bind_output_per_iter_from_env(Some("false")));
    assert!(!ort_bind_output_per_iter_from_env(Some("False")));
    assert!(!ort_bind_output_per_iter_from_env(Some("FALSE")));
}

#[test]
fn ort_bind_output_per_iter_other_values_enable() {
    assert!(ort_bind_output_per_iter_from_env(Some("1")));
    assert!(ort_bind_output_per_iter_from_env(Some("true")));
    assert!(ort_bind_output_per_iter_from_env(Some("yes")));
    assert!(ort_bind_output_per_iter_from_env(Some("on")));
    assert!(ort_bind_output_per_iter_from_env(Some("")));
}

#[test]
fn ort_bind_output_per_iter_trims_whitespace() {
    assert!(!ort_bind_output_per_iter_from_env(Some(" 0 ")));
    assert!(!ort_bind_output_per_iter_from_env(Some(" false ")));
    assert!(ort_bind_output_per_iter_from_env(Some(" 1 ")));
}
