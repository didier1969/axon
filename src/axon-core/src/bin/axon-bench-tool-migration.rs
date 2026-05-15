//! REQ-AXO-91490 (MIL-AXO-019 slice 0bis) — Bench harness for tool migration.
//!
//! Reads a JSONL corpus of tagged questions (default
//! `tests/data/tool_migration_questions.jsonl`) and runs each through the
//! live MCP endpoint via curl. Measures latency, returned-symbol set, and
//! precision@k / recall@k vs `expected_top_symbols` / `expected_concepts`.
//! Output : CSV per-question + aggregate summary.
//!
//! Usage :
//!   axon-bench-tool-migration --tool query [--corpus PATH] [--mode before|after]
//!                             [--baseline-csv PATH] [--human|--csv]
//!                             [--mcp-url http://127.0.0.1:44129/mcp]
//!
//! `--baseline-csv PATH` compares the current run against a saved CSV and
//! emits `pass|fail` per question (fail = precision drop > 5% or latency
//! regression > 50%). Without baseline, the run is the new baseline.

use std::fs;
use std::process::{Command, ExitCode};
use std::time::Instant;

use serde::Deserialize;
use serde_json::Value;

const DEFAULT_MCP_URL: &str = "http://127.0.0.1:44129/mcp";
const DEFAULT_CORPUS: &str = "tests/data/tool_migration_questions.jsonl";

#[derive(Debug, Deserialize)]
struct Question {
    id: String,
    category: String,
    tool: String,
    args: Value,
    #[serde(default)]
    expected_top_symbols: Vec<String>,
    #[serde(default)]
    expected_concepts: Vec<String>,
}

#[derive(Debug)]
struct Args {
    tool_filter: Option<String>,
    corpus: String,
    mode: String,
    baseline_csv: Option<String>,
    output: OutputMode,
    mcp_url: String,
}

#[derive(Debug, Clone, Copy)]
enum OutputMode {
    Csv,
    Human,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        let mut tool_filter: Option<String> = None;
        let mut corpus = DEFAULT_CORPUS.to_string();
        let mut mode = "after".to_string();
        let mut baseline_csv: Option<String> = None;
        let mut output = OutputMode::Csv;
        let mut mcp_url = DEFAULT_MCP_URL.to_string();
        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--tool" => {
                    tool_filter = Some(
                        raw.get(i + 1)
                            .ok_or("--tool requires value")?
                            .clone(),
                    );
                    i += 2;
                }
                "--corpus" => {
                    corpus = raw.get(i + 1).ok_or("--corpus requires path")?.clone();
                    i += 2;
                }
                "--mode" => {
                    mode = raw.get(i + 1).ok_or("--mode requires before|after")?.clone();
                    i += 2;
                }
                "--baseline-csv" => {
                    baseline_csv =
                        Some(raw.get(i + 1).ok_or("--baseline-csv requires path")?.clone());
                    i += 2;
                }
                "--mcp-url" => {
                    mcp_url = raw.get(i + 1).ok_or("--mcp-url requires url")?.clone();
                    i += 2;
                }
                "--human" => {
                    output = OutputMode::Human;
                    i += 1;
                }
                "--csv" => {
                    output = OutputMode::Csv;
                    i += 1;
                }
                "-h" | "--help" => {
                    println!("axon-bench-tool-migration --tool NAME [--corpus PATH] [--mode before|after] [--baseline-csv PATH] [--human|--csv] [--mcp-url URL]");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown arg: {}", other)),
            }
        }
        Ok(Self {
            tool_filter,
            corpus,
            mode,
            baseline_csv,
            output,
            mcp_url,
        })
    }
}

#[derive(Debug, Clone)]
struct QuestionResult {
    question_id: String,
    category: String,
    tool: String,
    latency_ms: u64,
    returned_symbols: Vec<String>,
    precision_at_5: f64,
    precision_at_10: f64,
    recall_at_5: f64,
    recall_at_10: f64,
    concept_hits: usize,
    error: Option<String>,
}

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("axon-bench-tool-migration: {}", e);
            return ExitCode::FAILURE;
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("axon-bench-tool-migration: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), String> {
    let body = fs::read_to_string(&args.corpus)
        .map_err(|e| format!("cannot read corpus {}: {}", args.corpus, e))?;
    let questions: Vec<Question> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("invalid jsonl line: {}", e)))
        .collect::<Result<_, _>>()?;

    let filtered: Vec<&Question> = questions
        .iter()
        .filter(|q| args.tool_filter.as_deref().map_or(true, |t| q.tool == t))
        .collect();

    let mut results: Vec<QuestionResult> = Vec::with_capacity(filtered.len());
    for q in &filtered {
        results.push(execute_question(q, &args.mcp_url));
    }

    let baseline = args
        .baseline_csv
        .as_ref()
        .and_then(|p| load_baseline_csv(p).ok())
        .unwrap_or_default();

    emit(&results, &baseline, args);
    Ok(())
}

fn execute_question(q: &Question, mcp_url: &str) -> QuestionResult {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": q.id,
        "params": {"name": q.tool, "arguments": q.args}
    });
    let body_str = body.to_string();
    let start = Instant::now();
    let output = Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "30",
            "-X",
            "POST",
            mcp_url,
            "-H",
            "Content-Type: application/json",
            "-d",
            &body_str,
        ])
        .output();
    let elapsed_ms = start.elapsed().as_millis() as u64;
    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout).to_string();
            let response: Value =
                serde_json::from_str(&raw).unwrap_or_else(|_| Value::Null);
            let returned = extract_symbol_ids(&response);
            let concept_hits = count_concept_hits(&response, &q.expected_concepts);
            let precision_at_5 = precision_at_k(&returned, &q.expected_top_symbols, 5);
            let precision_at_10 = precision_at_k(&returned, &q.expected_top_symbols, 10);
            let recall_at_5 = recall_at_k(&returned, &q.expected_top_symbols, 5);
            let recall_at_10 = recall_at_k(&returned, &q.expected_top_symbols, 10);
            QuestionResult {
                question_id: q.id.clone(),
                category: q.category.clone(),
                tool: q.tool.clone(),
                latency_ms: elapsed_ms,
                returned_symbols: returned,
                precision_at_5,
                precision_at_10,
                recall_at_5,
                recall_at_10,
                concept_hits,
                error: None,
            }
        }
        Ok(o) => QuestionResult {
            question_id: q.id.clone(),
            category: q.category.clone(),
            tool: q.tool.clone(),
            latency_ms: elapsed_ms,
            returned_symbols: Vec::new(),
            precision_at_5: 0.0,
            precision_at_10: 0.0,
            recall_at_5: 0.0,
            recall_at_10: 0.0,
            concept_hits: 0,
            error: Some(format!(
                "curl exited {}",
                o.status.code().unwrap_or(-1)
            )),
        },
        Err(e) => QuestionResult {
            question_id: q.id.clone(),
            category: q.category.clone(),
            tool: q.tool.clone(),
            latency_ms: elapsed_ms,
            returned_symbols: Vec::new(),
            precision_at_5: 0.0,
            precision_at_10: 0.0,
            recall_at_5: 0.0,
            recall_at_10: 0.0,
            concept_hits: 0,
            error: Some(format!("curl spawn failed: {}", e)),
        },
    }
}

fn extract_symbol_ids(response: &Value) -> Vec<String> {
    // Walk the response JSON; collect any string value located at a key
    // that resembles a symbol id field. Tool responses are heterogeneous
    // (`data.results[].id`, `data.cycles[].nodes[]`, `data.path[]`, ...).
    let mut out: Vec<String> = Vec::new();
    walk_collect_ids(response, &mut out);
    out
}

fn walk_collect_ids(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if matches!(
                    k.as_str(),
                    "id" | "target_id" | "source_id" | "caller_id" | "symbol_id" | "name"
                ) {
                    if let Some(s) = child.as_str() {
                        if !s.is_empty() {
                            out.push(s.to_string());
                        }
                    }
                }
                walk_collect_ids(child, out);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                walk_collect_ids(child, out);
            }
        }
        _ => {}
    }
}

fn count_concept_hits(response: &Value, concepts: &[String]) -> usize {
    if concepts.is_empty() {
        return 0;
    }
    let serialized = serde_json::to_string(response).unwrap_or_default();
    let lc = serialized.to_lowercase();
    concepts.iter().filter(|c| lc.contains(&c.to_lowercase())).count()
}

fn precision_at_k(returned: &[String], expected: &[String], k: usize) -> f64 {
    if expected.is_empty() || returned.is_empty() {
        return 0.0;
    }
    let top: std::collections::HashSet<&str> =
        returned.iter().take(k).map(String::as_str).collect();
    let hits = expected
        .iter()
        .filter(|e| top.iter().any(|r| r.contains(e.as_str())))
        .count();
    hits as f64 / (top.len() as f64).max(1.0)
}

fn recall_at_k(returned: &[String], expected: &[String], k: usize) -> f64 {
    if expected.is_empty() {
        return 0.0;
    }
    let top: std::collections::HashSet<&str> =
        returned.iter().take(k).map(String::as_str).collect();
    let hits = expected
        .iter()
        .filter(|e| top.iter().any(|r| r.contains(e.as_str())))
        .count();
    hits as f64 / expected.len() as f64
}

fn load_baseline_csv(path: &str) -> Result<std::collections::HashMap<String, (f64, u64)>, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("baseline read failed: {}", e))?;
    let mut map = std::collections::HashMap::new();
    for (i, line) in body.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            continue;
        }
        let id = cols[0].to_string();
        let prec: f64 = cols[3].parse().unwrap_or(0.0);
        let lat: u64 = cols[4].parse().unwrap_or(0);
        map.insert(id, (prec, lat));
    }
    Ok(map)
}

fn emit(
    results: &[QuestionResult],
    baseline: &std::collections::HashMap<String, (f64, u64)>,
    args: &Args,
) {
    let mut latencies: Vec<u64> = results.iter().map(|r| r.latency_ms).collect();
    latencies.sort_unstable();
    let p50 = latencies.get(latencies.len() / 2).copied().unwrap_or(0);
    let p99_idx = ((latencies.len() as f64) * 0.99) as usize;
    let p99 = latencies
        .get(p99_idx.min(latencies.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0);

    let avg_prec_5 =
        results.iter().map(|r| r.precision_at_5).sum::<f64>() / (results.len() as f64).max(1.0);
    let avg_prec_10 =
        results.iter().map(|r| r.precision_at_10).sum::<f64>() / (results.len() as f64).max(1.0);
    let avg_recall_5 =
        results.iter().map(|r| r.recall_at_5).sum::<f64>() / (results.len() as f64).max(1.0);
    let avg_recall_10 =
        results.iter().map(|r| r.recall_at_10).sum::<f64>() / (results.len() as f64).max(1.0);

    match args.output {
        OutputMode::Csv => {
            println!("id,category,tool,precision_at_5,latency_ms,precision_at_10,recall_at_5,recall_at_10,concept_hits,error,verdict");
            for r in results {
                let verdict = baseline_verdict(r, baseline);
                println!(
                    "{},{},{},{:.3},{},{:.3},{:.3},{:.3},{},{},{}",
                    r.question_id,
                    r.category,
                    r.tool,
                    r.precision_at_5,
                    r.latency_ms,
                    r.precision_at_10,
                    r.recall_at_5,
                    r.recall_at_10,
                    r.concept_hits,
                    r.error.as_deref().unwrap_or(""),
                    verdict
                );
            }
            eprintln!(
                "aggregate mode={} N={} p50_ms={} p99_ms={} prec5={:.3} prec10={:.3} recall5={:.3} recall10={:.3}",
                args.mode,
                results.len(),
                p50,
                p99,
                avg_prec_5,
                avg_prec_10,
                avg_recall_5,
                avg_recall_10,
            );
        }
        OutputMode::Human => {
            println!("# axon-bench-tool-migration mode={} N={}", args.mode, results.len());
            println!("latency p50 = {} ms", p50);
            println!("latency p99 = {} ms", p99);
            println!("precision@5 = {:.3}", avg_prec_5);
            println!("precision@10 = {:.3}", avg_prec_10);
            println!("recall@5 = {:.3}", avg_recall_5);
            println!("recall@10 = {:.3}", avg_recall_10);
            let cat_count: std::collections::HashMap<&str, usize> =
                results
                    .iter()
                    .fold(std::collections::HashMap::new(), |mut acc, r| {
                        *acc.entry(r.category.as_str()).or_insert(0) += 1;
                        acc
                    });
            for (cat, n) in cat_count {
                println!("  category {} : {} questions", cat, n);
            }
        }
    }
}

fn baseline_verdict(
    r: &QuestionResult,
    baseline: &std::collections::HashMap<String, (f64, u64)>,
) -> &'static str {
    match baseline.get(&r.question_id) {
        None => "new",
        Some(&(prev_prec, prev_lat)) => {
            let prec_drop = prev_prec - r.precision_at_5;
            let lat_regress = if prev_lat > 0 {
                (r.latency_ms as f64 - prev_lat as f64) / prev_lat as f64
            } else {
                0.0
            };
            if prec_drop > 0.05 || lat_regress > 0.5 {
                "fail"
            } else {
                "pass"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn fake_result(id: &str, prec: f64, lat: u64) -> QuestionResult {
        QuestionResult {
            question_id: id.to_string(),
            category: "single-lookup".to_string(),
            tool: "query".to_string(),
            latency_ms: lat,
            returned_symbols: Vec::new(),
            precision_at_5: prec,
            precision_at_10: prec,
            recall_at_5: 0.0,
            recall_at_10: 0.0,
            concept_hits: 0,
            error: None,
        }
    }

    #[test]
    fn precision_at_k_exact_match_returns_partial_precision() {
        // 1 hit out of top-3 returned = 1/3 precision
        let prec = precision_at_k(&s(&["axon_status", "graph_store", "foo"]), &s(&["axon_status"]), 3);
        assert!((prec - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn precision_at_k_zero_when_no_overlap() {
        let prec = precision_at_k(&s(&["foo", "bar"]), &s(&["baz"]), 5);
        assert_eq!(prec, 0.0);
    }

    #[test]
    fn precision_at_k_empty_returns_zero() {
        assert_eq!(precision_at_k(&[], &s(&["x"]), 5), 0.0);
        assert_eq!(precision_at_k(&s(&["x"]), &[], 5), 0.0);
    }

    #[test]
    fn precision_at_k_uses_substring_containment() {
        // returned IST id contains the expected name as a tail segment
        let returned = s(&["AXO::axon::src::axon-core::src::mcp::axon_status_status_impl"]);
        let prec = precision_at_k(&returned, &s(&["axon_status_status_impl"]), 5);
        assert!(prec > 0.0);
    }

    #[test]
    fn recall_at_k_full_when_all_expected_present() {
        let recall = recall_at_k(&s(&["a", "b", "c"]), &s(&["a", "b"]), 5);
        assert!((recall - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_zero_when_no_expected() {
        assert_eq!(recall_at_k(&s(&["a"]), &[], 5), 0.0);
    }

    #[test]
    fn extract_symbol_ids_walks_nested_data() {
        let v: Value = serde_json::from_str(
            r#"{"data":{"results":[{"id":"alpha"},{"id":"beta"}]}}"#,
        )
        .unwrap();
        let out = extract_symbol_ids(&v);
        assert!(out.contains(&"alpha".to_string()));
        assert!(out.contains(&"beta".to_string()));
    }

    #[test]
    fn extract_symbol_ids_returns_empty_for_unrelated_payload() {
        let v: Value = serde_json::from_str(r#"{"status":"ok","count":3}"#).unwrap();
        let out = extract_symbol_ids(&v);
        assert!(out.is_empty());
    }

    #[test]
    fn count_concept_hits_matches_case_insensitive() {
        let v: Value = serde_json::from_str(r#"{"text":"GraphRAG uses petgraph CSR"}"#).unwrap();
        let hits = count_concept_hits(&v, &s(&["graphrag", "petgraph", "missing"]));
        assert_eq!(hits, 2);
    }

    #[test]
    fn baseline_verdict_pass_when_precision_unchanged_and_latency_steady() {
        let r = fake_result("Q1", 0.8, 100);
        let mut base = std::collections::HashMap::new();
        base.insert("Q1".to_string(), (0.8, 100));
        assert_eq!(baseline_verdict(&r, &base), "pass");
    }

    #[test]
    fn baseline_verdict_fail_on_precision_drop_above_5pct() {
        let r = fake_result("Q1", 0.7, 100);
        let mut base = std::collections::HashMap::new();
        base.insert("Q1".to_string(), (0.8, 100));
        assert_eq!(baseline_verdict(&r, &base), "fail");
    }

    #[test]
    fn baseline_verdict_fail_on_latency_regression_above_50pct() {
        let r = fake_result("Q1", 0.8, 200);
        let mut base = std::collections::HashMap::new();
        base.insert("Q1".to_string(), (0.8, 100));
        assert_eq!(baseline_verdict(&r, &base), "fail");
    }

    #[test]
    fn baseline_verdict_new_when_question_absent_from_baseline() {
        let r = fake_result("Q42", 0.5, 50);
        let base = std::collections::HashMap::new();
        assert_eq!(baseline_verdict(&r, &base), "new");
    }
}
