//! REQ-AXO-158 (DEC-AXO-901650) — architectural drift continuous monitoring.
//!
//! Turns the one-shot `architectural_drift` probe into a *continuous* signal:
//! per recorded wave, the violation count for each monitored layer-pair is an
//! EWMA-smoothed score with a fixed-multiplier alert band. EWMA was chosen over
//! a Z-score because drift is a slow trend and a young/volatile corpus lacks the
//! stable variance a Z-score needs (DEC-AXO-901650). Samples persist to
//! `ist.drift_history` (append-only → heatmap + trend).
//!
//! This module is the pure engine (no PG); the `drift_history` MCP tool wires it
//! to persistence and reads it back. The monitored layer-pairs default to the
//! `forbidden` layer rules defined for REQ-AXO-157 (`structural_invariant`),
//! tying the two requirements together: 158 watches what 157 declares illegal.

use super::algorithms::layer_violations;
use super::snapshot::IstGraph;

/// EWMA smoothing factor ∈ (0,1]. Higher = more reactive to the latest wave.
pub const DEFAULT_ALPHA: f64 = 0.3;
/// Alert band multiplier: a score alerts when it exceeds `prev_ewma * k`.
pub const DEFAULT_K: f64 = 1.5;

/// Update the EWMA given the previous value (`None` on the first sample) and the
/// new score. First sample seeds the average with the score itself.
pub fn update_ewma(prev: Option<f64>, score: f64, alpha: f64) -> f64 {
    match prev {
        None => score,
        Some(e) => alpha * score + (1.0 - alpha) * e,
    }
}

/// A score is an alert when it exceeds the *previous* EWMA times `k` — a jump
/// above the smoothed trend. The first sample (no prior EWMA) never alerts; a
/// strictly positive score against a 0 baseline (drift onset) does.
pub fn is_alert(score: f64, prev_ewma: Option<f64>, k: f64) -> bool {
    match prev_ewma {
        None => false,
        Some(e) => score > 0.0 && score > e * k,
    }
}

/// Canonical persistence/heatmap key for a monitored boundary.
pub fn layer_pair_key(source_layer: &str, target_layer: &str) -> String {
    format!("{source_layer}->{target_layer}")
}

/// Current violation count for a forbidden `source_layer → target_layer`
/// boundary in the snapshot — the per-wave drift score. Reuses the same
/// `layer_violations` primitive as `architectural_drift`.
pub fn drift_score(graph: &IstGraph, source_layer: &str, target_layer: &str) -> u32 {
    let layer_def = vec![(source_layer, 0u32), (target_layer, 1u32)];
    layer_violations(graph, &layer_def).len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType};

    #[test]
    fn ewma_first_sample_seeds_with_score() {
        assert_eq!(update_ewma(None, 7.0, 0.3), 7.0);
    }

    #[test]
    fn ewma_blends_prev_and_new() {
        // 0.3*10 + 0.7*0 = 3.0
        assert!((update_ewma(Some(0.0), 10.0, 0.3) - 3.0).abs() < 1e-9);
        // 0.5*4 + 0.5*8 = 6.0
        assert!((update_ewma(Some(8.0), 4.0, 0.5) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn alert_rules() {
        // First sample never alerts.
        assert!(!is_alert(100.0, None, DEFAULT_K));
        // Jump above the band (12 > 5*1.5=7.5) alerts.
        assert!(is_alert(12.0, Some(5.0), DEFAULT_K));
        // Within the band (6 < 7.5) does not.
        assert!(!is_alert(6.0, Some(5.0), DEFAULT_K));
        // Drift onset from a clean baseline alerts.
        assert!(is_alert(3.0, Some(0.0), DEFAULT_K));
        // Staying clean does not.
        assert!(!is_alert(0.0, Some(0.0), DEFAULT_K));
    }

    #[test]
    fn pair_key_is_stable() {
        assert_eq!(layer_pair_key("AXO::core/", "AXO::mcp/"), "AXO::core/->AXO::mcp/");
    }

    fn node(id: &str, kind: NodeKind) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind,
            flags: NodeFlags::default(),
            complexity: None,
        }
    }
    fn edge(src: &str, tgt: &str) -> EdgeTriple {
        EdgeTriple {
            source: src.to_string(),
            target: tgt.to_string(),
            rel: RelationType::Calls,
        }
    }

    #[test]
    fn drift_score_counts_forbidden_crossings() {
        // Two core→mcp calls (violations) + one mcp→core (allowed).
        let nodes = vec![
            node("AXO::core/a.rs::f1", NodeKind::Function),
            node("AXO::core/a.rs::f2", NodeKind::Function),
            node("AXO::mcp/b.rs::g", NodeKind::Function),
        ];
        let edges = vec![
            edge("AXO::core/a.rs::f1", "AXO::mcp/b.rs::g"),
            edge("AXO::core/a.rs::f2", "AXO::mcp/b.rs::g"),
            edge("AXO::mcp/b.rs::g", "AXO::core/a.rs::f1"),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(drift_score(&g, "AXO::core/", "AXO::mcp/"), 2);
        assert_eq!(drift_score(&g, "AXO::mcp/", "AXO::core/"), 1);
    }
}
