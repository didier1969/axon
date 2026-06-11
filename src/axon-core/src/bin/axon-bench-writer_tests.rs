// REQ-AXO-260 / GUI-PRO-001 — argv parsing tests for axon-bench-writer.

use super::*;

fn args(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

#[test]
fn parse_defaults_when_no_args() {
    let parsed = Args::parse_from(&args(&[])).unwrap();
    assert_eq!(parsed.backend, "noop");
    assert_eq!(parsed.total, 10_000);
    assert_eq!(parsed.batch, 1_000);
    assert_eq!(parsed.dim, 1024);
    assert_eq!(parsed.project_code, "AXO");
    assert_eq!(parsed.label, "writer");
    assert!(matches!(parsed.output, OutputMode::Csv));
}

#[test]
fn parse_overrides_each_flag() {
    let parsed = Args::parse_from(&args(&[
        "--backend",
        "pgvector",
        "--total",
        "500",
        "--batch",
        "50",
        "--dim",
        "384",
        "--project-code",
        "BKS",
        "--model-id",
        "bge-base",
        "--label",
        "trial",
        "--human",
    ]))
    .unwrap();
    assert_eq!(parsed.backend, "pgvector");
    assert_eq!(parsed.total, 500);
    assert_eq!(parsed.batch, 50);
    assert_eq!(parsed.dim, 384);
    assert_eq!(parsed.project_code, "BKS");
    assert_eq!(parsed.model_id, "bge-base");
    assert_eq!(parsed.label, "trial");
    assert!(matches!(parsed.output, OutputMode::Human));
}

#[test]
fn parse_rejects_unknown_arg() {
    let res = Args::parse_from(&args(&["--unknown-flag", "x"]));
    assert!(res.is_err(), "unknown flag must error");
}

#[test]
fn parse_rejects_missing_value() {
    let res = Args::parse_from(&args(&["--backend"]));
    assert!(res.is_err(), "--backend without value must error");
    let res = Args::parse_from(&args(&["--total"]));
    assert!(res.is_err(), "--total without value must error");
}

#[test]
fn parse_rejects_non_numeric_total() {
    let res = Args::parse_from(&args(&["--total", "abc"]));
    assert!(res.is_err(), "non-numeric --total must error");
}

#[test]
fn chrono_now_ms_returns_positive_value() {
    let v = chrono_now_ms();
    assert!(
        v > 1_700_000_000_000i64,
        "epoch ms must be after 2023-11; got {v}"
    );
}
