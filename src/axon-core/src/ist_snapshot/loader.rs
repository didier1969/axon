// REQ-AXO-91485 — cold-load IstGraph from PG.
//
// One round trip on ist.symbol (nodes) + one on ist.edge (edges), both
// scoped to a single project_code. Read-only ; no DDL ; no incremental sync
// (that lives in REQ-AXO-91487).

use std::time::Instant;

use crate::ist_snapshot::snapshot::{
    EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
};

/// Diagnostics returned alongside the snapshot. Surfaced by the bench binary
/// and by future tools that want to expose load metrics.
#[derive(Clone, Debug)]
pub struct LoadStats {
    pub project_code: String,
    pub nodes_loaded: usize,
    pub edges_loaded: usize,
    pub load_ms: u64,
    pub approximate_bytes: usize,
}

/// Trait abstracting the SQL surface so the loader can be unit-tested with
/// in-memory fixtures (see snapshot.rs tests) instead of a live PG. The
/// `query_json` method matches the existing `GraphStore::query_json` shape :
/// returns a JSON array of array-of-strings rows.
pub trait JsonSqlStore {
    fn query_json(&self, sql: &str) -> Result<String, String>;
}

const NODE_SQL: &str = "SELECT id, kind, project_code, tested::text, is_public::text, is_nif::text, is_unsafe::text, name FROM ist.symbol WHERE project_code = '{P}'";
const EDGE_SQL: &str =
    "SELECT source_id, target_id, relation_type FROM ist.edge WHERE project_code = '{P}'";

/// Cold-load one project's snapshot. `project_code` is escaped at the
/// call site by replacing `'` with `''` ; callers must not pass arbitrary
/// untrusted input here. The function returns `Err` only if both queries
/// fail or the JSON cannot be parsed ; partial data (e.g. zero edges) is
/// valid and yields a snapshot.
// REQ-AXO-902005 — `?Sized` so the async serve-stale refresh can pass a
// `&dyn JsonSqlStore` trait object (boxed/Arc'd store handle); all existing
// `&ConcreteStore` callers are unaffected.
pub fn load_snapshot<S: JsonSqlStore + ?Sized>(
    store: &S,
    project_code: &str,
) -> Result<(IstGraph, LoadStats), String> {
    let safe_code = project_code.replace('\'', "''");
    let started = Instant::now();

    let node_sql = NODE_SQL.replace("{P}", &safe_code);
    let node_rows = parse_rows(store.query_json(&node_sql)?)?;
    let nodes: Vec<NodeRecord> = node_rows
        .into_iter()
        .filter_map(|row| {
            if row.len() < 7 {
                return None;
            }
            // REQ-AXO-901970 — `name` (col 8) carries the canonical display name;
            // fall back to the id suffix if absent (older snapshots / NULL name).
            let name = row
                .get(7)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| row[0].rsplit("::").next().unwrap_or(&row[0]).to_string());
            Some(NodeRecord {
                id: row[0].clone(),
                name,
                kind: NodeKind::from_db(&row[1]),
                project_code: row[2].clone(),
                flags: NodeFlags::new(
                    parse_bool(&row[3]),
                    parse_bool(&row[4]),
                    parse_bool(&row[5]),
                    parse_bool(&row[6]),
                ),
            })
        })
        .collect();

    let edge_sql = EDGE_SQL.replace("{P}", &safe_code);
    let edge_rows = parse_rows(store.query_json(&edge_sql)?)?;
    let edges: Vec<EdgeTriple> = edge_rows
        .into_iter()
        .filter_map(|row| {
            if row.len() < 3 {
                return None;
            }
            Some(EdgeTriple {
                source: row[0].clone(),
                target: row[1].clone(),
                rel: RelationType::from_db(&row[2]),
            })
        })
        .collect();

    let nodes_count = nodes.len();
    let edges_count = edges.len();
    let graph = IstGraph::build(nodes, edges);
    let stats = LoadStats {
        project_code: project_code.to_string(),
        nodes_loaded: nodes_count,
        edges_loaded: edges_count,
        load_ms: started.elapsed().as_millis() as u64,
        approximate_bytes: graph.approximate_bytes(),
    };
    Ok((graph, stats))
}

fn parse_rows(json: String) -> Result<Vec<Vec<String>>, String> {
    serde_json::from_str::<Vec<Vec<String>>>(&json)
        .map_err(|e| format!("ist_snapshot loader: row parse failed: {}", e))
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "t" | "true" | "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeStore {
        nodes_json: String,
        edges_json: String,
        calls: RefCell<Vec<String>>,
    }

    impl JsonSqlStore for FakeStore {
        fn query_json(&self, sql: &str) -> Result<String, String> {
            self.calls.borrow_mut().push(sql.to_string());
            if sql.contains("ist.symbol") {
                Ok(self.nodes_json.clone())
            } else if sql.contains("ist.edge") {
                Ok(self.edges_json.clone())
            } else {
                Err(format!("unexpected sql: {}", sql))
            }
        }
    }

    #[test]
    fn loader_builds_graph_from_fixture_rows() {
        let store = FakeStore {
            nodes_json: r#"[
              ["AXO::a", "function", "AXO", "f", "t", "f", "f"],
              ["AXO::b", "function", "AXO", "f", "t", "f", "f"]
            ]"#
            .to_string(),
            edges_json: r#"[
              ["AXO::a", "AXO::b", "CALLS"]
            ]"#
            .to_string(),
            calls: RefCell::new(Vec::new()),
        };
        let (g, stats) = load_snapshot(&store, "AXO").unwrap();
        assert_eq!(stats.project_code, "AXO");
        assert_eq!(stats.nodes_loaded, 2);
        assert_eq!(stats.edges_loaded, 1);
        let a = g.index_of("AXO::a").unwrap();
        let b = g.index_of("AXO::b").unwrap();
        let fwd: Vec<_> = g.forward_neighbors(a).map(|(t, _)| t).collect();
        assert_eq!(fwd, vec![b]);
    }

    #[test]
    fn loader_resolves_synthetic_call_target_via_unique_name() {
        // REQ-AXO-140 — end-to-end via the production load_snapshot path: a CALLS
        // edge whose target is a synthetic `caller_file::callee` (no node of its
        // own) resolves to the UNIQUE canonical method node of that name in the
        // built RAM graph. Proves the IstGraph::build resolution fires on
        // PG-loaded rows, not just on hand-built unit fixtures.
        let store = FakeStore {
            nodes_json: r#"[
              ["AXO::a.rs::caller", "function", "AXO", "f", "t", "f", "f"],
              ["AXO::b.rs::callee", "method", "AXO", "f", "t", "f", "f"]
            ]"#
            .to_string(),
            // Synthetic target `AXO::a.rs::callee` — no such node; last `::` segment `callee`.
            edges_json: r#"[
              ["AXO::a.rs::caller", "AXO::a.rs::callee", "CALLS"]
            ]"#
            .to_string(),
            calls: RefCell::new(Vec::new()),
        };
        let (g, _stats) = load_snapshot(&store, "AXO").unwrap();
        let caller = g.index_of("AXO::a.rs::caller").unwrap();
        let callee = g.index_of("AXO::b.rs::callee").unwrap();
        assert_eq!(
            g.index_of("AXO::a.rs::callee"),
            None,
            "synthetic target must NOT become a phantom node"
        );
        let rev: Vec<_> = g.reverse_neighbors(callee).map(|(s, _)| s).collect();
        assert_eq!(
            rev,
            vec![caller],
            "synthetic CALLS resolved to the canonical callee in the loaded graph"
        );
    }

    #[test]
    fn loader_tolerates_empty_edges() {
        let store = FakeStore {
            nodes_json: r#"[
              ["AXO::a", "function", "AXO", "f", "t", "f", "f"]
            ]"#
            .to_string(),
            edges_json: "[]".to_string(),
            calls: RefCell::new(Vec::new()),
        };
        let (g, stats) = load_snapshot(&store, "AXO").unwrap();
        assert_eq!(stats.nodes_loaded, 1);
        assert_eq!(stats.edges_loaded, 0);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn loader_escapes_single_quote_in_project_code() {
        let store = FakeStore {
            nodes_json: "[]".to_string(),
            edges_json: "[]".to_string(),
            calls: RefCell::new(Vec::new()),
        };
        let _ = load_snapshot(&store, "A'B").unwrap();
        let calls = store.calls.borrow();
        for call in calls.iter() {
            assert!(
                call.contains("'A''B'"),
                "expected escaped quote, got: {}",
                call
            );
        }
    }

    #[test]
    fn loader_propagates_query_error() {
        struct BrokenStore;
        impl JsonSqlStore for BrokenStore {
            fn query_json(&self, _sql: &str) -> Result<String, String> {
                Err("connection lost".to_string())
            }
        }
        let outcome = load_snapshot(&BrokenStore, "AXO");
        let err = match outcome {
            Ok(_) => panic!("expected error from BrokenStore"),
            Err(e) => e,
        };
        assert!(err.contains("connection lost"), "unexpected: {}", err);
    }

    #[test]
    fn parse_bool_accepts_postgres_text_forms() {
        for truthy in ["t", "T", "true", "TRUE", "1"] {
            assert!(parse_bool(truthy), "expected truthy: {}", truthy);
        }
        for falsy in ["f", "false", "0", ""] {
            assert!(!parse_bool(falsy), "expected falsy: {:?}", falsy);
        }
    }
}
