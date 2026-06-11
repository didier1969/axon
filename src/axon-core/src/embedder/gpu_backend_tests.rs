// REQ-AXO-262 / GUI-PRO-001 — sibling tests for gpu_backend helpers.

use super::{
    bucket_up, cuda_tf32_enabled_from_env, ort_bind_output_per_iter_from_env,
    ort_memory_pattern_enabled_from_env, parse_seq_buckets_from_env, DEFAULT_SEQ_BUCKETS,
};

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

// ── REQ-AXO-262 — seq-length bucketing tests ─────────────────────────────

#[test]
fn bucket_up_rounds_to_next_bucket_in_list() {
    let buckets = [128usize, 256, 384, 512];
    assert_eq!(bucket_up(1, &buckets), 128);
    assert_eq!(bucket_up(127, &buckets), 128);
    assert_eq!(bucket_up(128, &buckets), 128);
    assert_eq!(bucket_up(129, &buckets), 256);
    assert_eq!(bucket_up(256, &buckets), 256);
    assert_eq!(bucket_up(300, &buckets), 384);
    assert_eq!(bucket_up(512, &buckets), 512);
}

#[test]
fn bucket_up_clamps_oversize_to_largest_bucket() {
    let buckets = [128usize, 256, 512];
    assert_eq!(bucket_up(1000, &buckets), 512);
}

#[test]
fn bucket_up_passthrough_when_buckets_empty() {
    let buckets: [usize; 0] = [];
    assert_eq!(bucket_up(73, &buckets), 73);
    assert_eq!(bucket_up(0, &buckets), 0);
}

#[test]
fn parse_seq_buckets_defaults_to_canonical_list() {
    assert_eq!(
        parse_seq_buckets_from_env(None),
        DEFAULT_SEQ_BUCKETS.to_vec()
    );
}

#[test]
fn parse_seq_buckets_explicit_disable_returns_empty() {
    assert!(parse_seq_buckets_from_env(Some("")).is_empty());
    assert!(parse_seq_buckets_from_env(Some("   ")).is_empty());
    assert!(parse_seq_buckets_from_env(Some("0")).is_empty());
    assert!(parse_seq_buckets_from_env(Some("off")).is_empty());
    assert!(parse_seq_buckets_from_env(Some("OFF")).is_empty());
    assert!(parse_seq_buckets_from_env(Some("none")).is_empty());
}

#[test]
fn parse_seq_buckets_normalizes_input() {
    assert_eq!(
        parse_seq_buckets_from_env(Some("256, 128, 512, 256, 384")),
        vec![128, 256, 384, 512]
    );
    assert_eq!(
        parse_seq_buckets_from_env(Some("  128 , 256  ")),
        vec![128, 256]
    );
}

#[test]
fn parse_seq_buckets_skips_non_numeric_and_zero() {
    assert_eq!(
        parse_seq_buckets_from_env(Some("128,abc,0,256")),
        vec![128, 256]
    );
}

#[test]
fn parse_seq_buckets_falls_back_to_default_if_all_invalid() {
    assert_eq!(
        parse_seq_buckets_from_env(Some("abc,xyz")),
        DEFAULT_SEQ_BUCKETS.to_vec()
    );
}

#[test]
fn parse_seq_buckets_accepts_single_fixed_value() {
    // Useful for "force a single TRT engine shape" experiments.
    assert_eq!(parse_seq_buckets_from_env(Some("512")), vec![512]);
}

// ── REQ-AXO-262 — TF32 default ON regression tests ───────────────────────

#[test]
fn cuda_tf32_default_is_on() {
    assert!(cuda_tf32_enabled_from_env(None));
}

#[test]
fn cuda_tf32_explicit_disable_keywords() {
    assert!(!cuda_tf32_enabled_from_env(Some("0")));
    assert!(!cuda_tf32_enabled_from_env(Some("false")));
    assert!(!cuda_tf32_enabled_from_env(Some("False")));
    assert!(!cuda_tf32_enabled_from_env(Some("FALSE")));
    assert!(!cuda_tf32_enabled_from_env(Some("no")));
    assert!(!cuda_tf32_enabled_from_env(Some("off")));
}

#[test]
fn cuda_tf32_other_values_enable() {
    assert!(cuda_tf32_enabled_from_env(Some("1")));
    assert!(cuda_tf32_enabled_from_env(Some("true")));
    assert!(cuda_tf32_enabled_from_env(Some("yes")));
    assert!(cuda_tf32_enabled_from_env(Some("on")));
    assert!(cuda_tf32_enabled_from_env(Some("")));
}

#[test]
fn cuda_tf32_trims_whitespace() {
    assert!(!cuda_tf32_enabled_from_env(Some(" 0 ")));
    assert!(!cuda_tf32_enabled_from_env(Some(" false ")));
    assert!(cuda_tf32_enabled_from_env(Some(" 1 ")));
}
