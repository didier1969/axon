//! REQ-AXO-219 — layered-retrieval band sizing / truncation / collection helpers
//! extracted from the `tools_context.rs` god-file (APoSD deep-module split).
//! Associated functions on `McpServer`; behavior-preserving move, `Self::…`
//! call sites unchanged. They cap, truncate and collect the intent / code /
//! recent bands for `retrieve_context_layered`.

use super::super::McpServer;
use super::util::estimate_tokens;
use serde_json::{json, Value};

impl McpServer {
    pub(super) fn layered_band_max_tokens(args: &Value, band: &str, default: usize) -> usize {
        args.get("bands")
            .and_then(|b| b.get(band))
            .and_then(|cfg| cfg.get("max_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(default)
            .max(50) // hard floor so we always emit at least the band scaffold
    }

    // intent_band truncation: drop rows from the back, prioritising
    // requirements > decisions > concepts (the closer-to-action signal first).
    // Returns (concepts_kept, decisions_kept, requirements_kept, tokens_post, overflow_count).
    pub(super) fn truncate_intent_band(
        concepts: Vec<Value>,
        decisions: Vec<Value>,
        requirements: Vec<Value>,
        budget: usize,
    ) -> (Vec<Value>, Vec<Value>, Vec<Value>, usize, usize) {
        let measure = |c: &[Value], d: &[Value], r: &[Value]| -> usize {
            let s = serde_json::to_string(&json!({
                "concepts": c, "decisions": d, "requirements": r
            }))
            .unwrap_or_default();
            estimate_tokens(&[&s])
        };
        let pre = measure(&concepts, &decisions, &requirements);
        if pre <= budget {
            return (concepts, decisions, requirements, pre, 0);
        }
        // Truncate in reverse priority: concepts first, then decisions, then requirements.
        let mut c = concepts.clone();
        let mut d = decisions.clone();
        let mut r = requirements.clone();
        let initial_total = c.len() + d.len() + r.len();
        while measure(&c, &d, &r) > budget {
            if !c.is_empty() {
                c.pop();
            } else if !d.is_empty() {
                d.pop();
            } else if !r.is_empty() {
                r.pop();
            } else {
                break;
            }
        }
        let kept_total = c.len() + d.len() + r.len();
        let dropped = initial_total - kept_total;
        let post = measure(&c, &d, &r);
        (c, d, r, post, dropped)
    }

    // code_band truncation: drop chunks from the back (the lowest-ranked).
    pub(super) fn truncate_chunks_band(chunks: Vec<Value>, budget: usize) -> (Vec<Value>, usize, usize) {
        let pre_text = serde_json::to_string(&chunks).unwrap_or_default();
        let pre = estimate_tokens(&[&pre_text]);
        if pre <= budget {
            return (chunks, pre, 0);
        }
        let mut kept = chunks;
        let initial = kept.len();
        while !kept.is_empty()
            && estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]) > budget
        {
            kept.pop();
        }
        let dropped = initial - kept.len();
        let post = estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]);
        (kept, post, dropped)
    }

    // recent_band truncation: drop oldest git_recent_edits entries first.
    pub(super) fn truncate_recent_band(mut band: Value, budget: usize) -> (Value, usize, usize) {
        let entries: Vec<Value> = band
            .get_mut("git_recent_edits")
            .and_then(|v| v.as_array_mut())
            .map(|a| std::mem::take(a))
            .unwrap_or_default();
        let pre_text = serde_json::to_string(&entries).unwrap_or_default();
        let pre = estimate_tokens(&[&pre_text]);
        if pre <= budget {
            // Restore entries unchanged + recompute tokens_used to be safe.
            band["git_recent_edits"] = json!(entries);
            band["tokens_used"] = json!(pre);
            return (band, pre, 0);
        }
        let mut kept = entries;
        let initial = kept.len();
        while !kept.is_empty()
            && estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]) > budget
        {
            // Entries are sorted newest-first; pop drops the oldest.
            kept.pop();
        }
        let dropped = initial - kept.len();
        let post = estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]);
        band["git_recent_edits"] = json!(kept);
        band["tokens_used"] = json!(post);
        (band, post, dropped)
    }

    // REQ-AXO-264 A6 v1 — recent_band collector.
    //
    // Runs `git log --since=24.hours --name-only --pretty=format:%H\x01%ct\x01%s`
    // in the resolved project root. Each commit emits its hash/timestamp/
    // subject followed by changed paths. We collect (file, last_commit_ts,
    // last_subject) keyed by file (most recent commit wins).
    //
    // Returns a stable JSON contract:
    //   { git_recent_edits: [...], current_focus: ..., tokens_used: N,
    //     status: "ok" | "no_project_root" | "git_error", ... }
    //
    // If git fails or there's no project root, returns an empty band tagged
    // with the failure reason so LLM clients can act on it.
    pub(crate) fn collect_recent_band(project_root: Option<&str>) -> Value {
        let Some(root) = project_root else {
            return json!({
                "git_recent_edits": [],
                "current_focus": Value::Null,
                "tokens_used": 0,
                "status": "no_project_root",
            });
        };
        if !std::path::Path::new(root).is_dir() {
            return json!({
                "git_recent_edits": [],
                "current_focus": Value::Null,
                "tokens_used": 0,
                "status": "no_project_root",
                "reason": format!("path not a directory: {root}"),
            });
        }

        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("log")
            .arg("--since=24.hours")
            .arg("--name-only")
            .arg("--pretty=format:%H\x01%ct\x01%s")
            .output();

        let stdout = match output {
            Ok(o) if o.status.success() => o.stdout,
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                return json!({
                    "git_recent_edits": [],
                    "current_focus": Value::Null,
                    "tokens_used": 0,
                    "status": "git_error",
                    "reason": stderr.lines().next().unwrap_or("").to_string(),
                });
            }
            Err(err) => {
                return json!({
                    "git_recent_edits": [],
                    "current_focus": Value::Null,
                    "tokens_used": 0,
                    "status": "git_error",
                    "reason": err.to_string(),
                });
            }
        };

        let text = String::from_utf8_lossy(&stdout);
        let mut by_file: std::collections::BTreeMap<String, (i64, String, String)> =
            std::collections::BTreeMap::new();
        let mut current_hash = String::new();
        let mut current_ts: i64 = 0;
        let mut current_subject = String::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if line.contains('\x01') {
                let mut parts = line.splitn(3, '\x01');
                current_hash = parts.next().unwrap_or("").to_string();
                current_ts = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                current_subject = parts.next().unwrap_or("").to_string();
            } else if !current_hash.is_empty() {
                // git log already emits files newest first; only insert if
                // this is the first time we see this path (preserves the
                // most recent commit per file).
                by_file
                    .entry(line.to_string())
                    .or_insert_with(|| (current_ts, current_hash.clone(), current_subject.clone()));
            }
        }

        let mut entries: Vec<Value> = by_file
            .into_iter()
            .map(|(file, (ts, hash, subject))| {
                json!({
                    "file": file,
                    "last_commit_ts": ts,
                    "last_commit_hash": hash,
                    "last_commit_subject": subject,
                })
            })
            .collect();
        // Sort by recency (newest first).
        entries.sort_by(|a, b| {
            b["last_commit_ts"]
                .as_i64()
                .unwrap_or(0)
                .cmp(&a["last_commit_ts"].as_i64().unwrap_or(0))
        });
        let recent_text = serde_json::to_string(&entries).unwrap_or_default();
        let tokens_used = estimate_tokens(&[&recent_text]);

        // current_focus: best-effort cwd hint (the dir we're in, relative to
        // the project root if possible). Does NOT touch open editor state.
        let current_focus = std::env::current_dir().ok().map(|cwd| {
            let cwd_str = cwd.to_string_lossy().to_string();
            let rel = cwd_str
                .strip_prefix(root)
                .map(|s| s.trim_start_matches('/').to_string())
                .unwrap_or_else(|| cwd_str.clone());
            json!({ "cwd": cwd_str, "relative_to_project": rel })
        });

        json!({
            "git_recent_edits": entries,
            "current_focus": current_focus,
            "tokens_used": tokens_used,
            "status": "ok",
            "window": "24h",
        })
    }
}
