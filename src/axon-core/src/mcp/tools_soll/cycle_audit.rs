// REQ-AXO-91492 (MIL-AXO-019 slice X) — soll_acyclic_audit tool.
//
// Read-only scan : enumerate strongly-connected components (size > 1) and
// self-loops in the SOLL graph for a single project. Wraps
// `SollSnapshot::cycle_sets()` so the tool can be exposed without re-running
// Tarjan in the handler. Used as the audit pre-requisite for DEC-AXO-098
// (cycle validation pre-write in `soll_manager(action=link)`). Pure read ;
// no mutation ; no DDL.

use serde_json::{json, Value};

use super::*;

impl McpServer {
    pub(crate) fn axon_soll_acyclic_audit(&self, args: &Value) -> Option<Value> {
        let project_arg = args.get("project_code").and_then(|v| v.as_str());
        let resolved = match project_arg {
            Some(code) => match self.resolve_project_code(code) {
                Ok(c) => c,
                Err(_) => {
                    return Some(self.wrong_project_scope_response(code, "soll_acyclic_audit"));
                }
            },
            None => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "soll_acyclic_audit requires a project_code (e.g. AXO)."
                    }],
                    "isError": true,
                    "data": {
                        "status": "missing_project_code",
                        "parameter_repair": {
                            "invalid_field": "project_code",
                            "tool": "soll_acyclic_audit",
                            "follow_up_tools": ["project_registry_lookup", "help"],
                            "hint": "supply the canonical 3-letter project code (e.g. AXO)"
                        }
                    }
                }));
            }
        };

        let snapshot = match self.soll_cache().snapshot(&resolved) {
            Ok(s) => s,
            Err(e) => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("soll_acyclic_audit snapshot error: {}", e)
                    }],
                    "isError": true,
                    "data": {
                        "status": "internal_error",
                        "project_code": resolved,
                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>(),
                        "parameter_repair": {
                            "invalid_field": "soll_cache_snapshot",
                            "tool": "soll_acyclic_audit",
                            "follow_up_tools": ["status", "soll_query_context"],
                            "hint": "snapshot computation failed; check `status` and retry"
                        }
                    }
                }));
            }
        };

        let cycle_sets = snapshot.cycle_sets();
        Some(build_audit_response(
            &resolved,
            snapshot.node_count(),
            snapshot.edge_count(),
            &cycle_sets,
        ))
    }
}

fn build_audit_response(
    project_code: &str,
    node_count: usize,
    edge_count: usize,
    cycle_sets: &[std::collections::HashSet<String>],
) -> Value {
    let cycle_count = cycle_sets.len();
    let mut cycles_json: Vec<Value> = Vec::with_capacity(cycle_count);
    for set in cycle_sets {
        let mut ids: Vec<String> = set.iter().cloned().collect();
        ids.sort();
        cycles_json.push(json!({
            "size": ids.len(),
            "nodes": ids
        }));
    }
    let summary_text = if cycle_count == 0 {
        format!(
            "SOLL acyclic audit ok for {} : 0 SCC>1 and 0 self-loop detected ({} nodes, {} edges).",
            project_code, node_count, edge_count
        )
    } else {
        format!(
            "SOLL acyclic audit for {} : {} cycle(s) detected (SCC>1 or self-loop). DEC-AXO-098 cycle validator activation requires these to be 0.",
            project_code, cycle_count
        )
    };
    let status_str = if cycle_count == 0 {
        "ok"
    } else {
        "cycles_detected"
    };
    json!({
        "content": [{ "type": "text", "text": summary_text }],
        "data": {
            "status": status_str,
            "project_code": project_code,
            "node_count": node_count,
            "edge_count": edge_count,
            "cycle_count": cycle_count,
            "cycles": cycles_json,
            "operator_guidance": {
                "problem_class": if cycle_count == 0 { "none" } else { "cycle_present_in_soll" },
                "follow_up_tools": ["soll_validate", "soll_query_context", "soll_manager"],
                "next_action": if cycle_count == 0 {
                    "no action — eligible to activate DEC-AXO-098 cycle validator on link path"
                } else {
                    "review each cycle ; archive / re-link offending edges before activating cycle validation"
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn cycle(nodes: &[&str]) -> HashSet<String> {
        nodes.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn build_response_zero_cycles_emits_ok_status() {
        let resp = build_audit_response("AXO", 350, 700, &[]);
        assert_eq!(resp.pointer("/data/status").and_then(Value::as_str), Some("ok"));
        assert_eq!(
            resp.pointer("/data/cycle_count").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            resp.pointer("/data/operator_guidance/problem_class")
                .and_then(Value::as_str),
            Some("none")
        );
    }

    #[test]
    fn build_response_detects_cycles_and_sorts_nodes() {
        let cycles = vec![cycle(&["REQ-B", "REQ-A"]), cycle(&["DEC-X"])];
        let resp = build_audit_response("AXO", 350, 700, &cycles);
        assert_eq!(
            resp.pointer("/data/status").and_then(Value::as_str),
            Some("cycles_detected")
        );
        assert_eq!(
            resp.pointer("/data/cycle_count").and_then(Value::as_u64),
            Some(2)
        );
        let first_nodes = resp
            .pointer("/data/cycles/0/nodes")
            .and_then(Value::as_array)
            .unwrap();
        let first_strs: Vec<&str> = first_nodes.iter().filter_map(Value::as_str).collect();
        assert_eq!(first_strs, vec!["REQ-A", "REQ-B"]);
    }

    #[test]
    fn build_response_size_reflects_nodes_in_cycle() {
        let cycles = vec![cycle(&["A", "B", "C"])];
        let resp = build_audit_response("AXO", 10, 20, &cycles);
        assert_eq!(
            resp.pointer("/data/cycles/0/size").and_then(Value::as_u64),
            Some(3)
        );
    }

    #[test]
    fn build_response_message_mentions_dec_098_when_cycles_present() {
        let cycles = vec![cycle(&["A", "B"])];
        let resp = build_audit_response("AXO", 5, 10, &cycles);
        let text = resp
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(text.contains("DEC-AXO-098"));
    }
}
