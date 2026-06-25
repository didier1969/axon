//! REQ-AXO-219 — semantic service-pressure helpers extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Pure associated
//! functions on `McpServer`; behavior-preserving move, `Self::…` call sites
//! unchanged. They decide whether the corpus-wide semantic ANN may run, wait
//! (bounded) for recovery, and surface a TRANSIENT_UNAVAILABILITY notice when
//! it is skipped (PIL-AXO-002).

use super::super::McpServer;
use super::DEFAULT_WAIT_FOR_SEMANTIC_MS;
use crate::service_guard::ServicePressure;
use serde_json::{json, Value};

impl McpServer {
    pub(super) fn semantic_corpus_pressure_ok(pressure: ServicePressure) -> bool {
        matches!(
            pressure,
            ServicePressure::Healthy | ServicePressure::Recovering
        )
    }

    /// REQ-AXO-902023 tier C.1 — normalize the `wait_for_semantic` arg: an
    /// explicit ms budget, or the bool shorthand `true` → DEFAULT budget. Any
    /// other shape (false / string / 0 handled downstream) yields None = no wait.
    pub(super) fn parse_wait_for_semantic(value: &Value) -> Option<u64> {
        if let Some(ms) = value.as_u64() {
            return Some(ms);
        }
        if value.as_bool() == Some(true) {
            return Some(DEFAULT_WAIT_FOR_SEMANTIC_MS);
        }
        None
    }

    /// REQ-AXO-902023 tier C.1 — bounded wait for semantic pressure recovery.
    /// Samples pressure once; if no budget or it already permits the corpus ANN,
    /// returns immediately (today's behavior). Otherwise polls every `step_ms`
    /// (clamped to the remaining budget) until pressure recovers or the budget is
    /// exhausted, returning the last sample plus the total waited. `sample` and
    /// `sleep` are injected so the loop is unit-testable without wall-clock.
    pub(super) fn resolve_pressure_with_wait(
        wait_ms: Option<u64>,
        step_ms: u64,
        mut sample: impl FnMut() -> ServicePressure,
        mut sleep: impl FnMut(u64),
    ) -> (ServicePressure, u64) {
        let first = sample();
        let budget = match wait_ms {
            Some(budget) if budget > 0 => budget,
            _ => return (first, 0),
        };
        if Self::semantic_corpus_pressure_ok(first) {
            return (first, 0);
        }
        let step = step_ms.max(1);
        let mut waited = 0u64;
        let mut last = first;
        while waited < budget {
            let this_step = step.min(budget - waited);
            sleep(this_step);
            waited += this_step;
            last = sample();
            if Self::semantic_corpus_pressure_ok(last) {
                break;
            }
        }
        (last, waited)
    }

    /// signal. When the corpus-wide semantic chunk search is skipped under service
    /// pressure / vector backlog, the contract (PIL-AXO-002) demands the
    /// degradation be surfaced as a distinct TRANSIENT_UNAVAILABILITY notice — not
    /// buried in `excluded_because` where the agent silently gets a lexical-only
    /// answer and drifts back to grep (mcp_feedback #11). Returns None for the
    /// caller's own `semantic=lexical|off` (a choice, not a degradation).
    pub(super) fn build_degradation_notice(
        degraded_reason: Option<&str>,
        pressure: ServicePressure,
        rerank_applied: bool,
    ) -> Option<Value> {
        let reason = degraded_reason?;
        if !reason.starts_with("semantic_chunk_search_skipped") {
            return None;
        }
        let backlog = reason.contains("vector_backlog");
        // REQ-AXO-902018 tier B — when the cheap candidate re-rank ran, the result
        // is materially better than lexical-only; say so instead of overstating
        // the degradation.
        let (impact, remediation) = if rerank_applied {
            (
                "Corpus-wide semantic search was skipped under service pressure, but the structural + lexical candidates were re-ranked with a single query embedding (lite semantic). Reliability is below a full ANN sweep yet well above lexical-only.",
                "Transient: results are usable now; retry shortly for a full corpus sweep, or pass semantic=semantic to force it.",
            )
        } else if backlog {
            (
                "Corpus-wide semantic chunk search was skipped — evidence ranks by lexical + structural signals only. Reliability is reduced for capability / behaviour questions ('which component does X?').",
                "Vector backlog: results improve once indexing catches up. Structural tools (query / inspect / impact / path) are unaffected.",
            )
        } else {
            (
                "Corpus-wide semantic chunk search was skipped — evidence ranks by lexical + structural signals only. Reliability is reduced for capability / behaviour questions ('which component does X?').",
                "Transient under service pressure: retry shortly. Meanwhile rely on structural tools (query / inspect / impact / path); pass semantic=semantic to force the embed.",
            )
        };
        Some(json!({
            "degraded": true,
            "class": "TRANSIENT_UNAVAILABILITY",
            "reason": reason,
            "service_pressure": format!("{pressure:?}"),
            "semantic_rerank_applied": rerank_applied,
            "impact": impact,
            "remediation": remediation,
        }))
    }
}
