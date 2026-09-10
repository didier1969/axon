//! REQ-AXO-902392 — Prometheus metrics exporter for Axon.
//!
//! Exposes an aggregated `/metrics` endpoint on the Axon Brain HTTP port (44129)
//! allowing fleet observability tools (Prometheus, Grafana, alerts) to monitor
//! Axon health without relying on conversational MCP invocations.
//!
//! Invariants:
//! - Strict `not_armed` contract: when a metric (such as `axon_b2_cpu_fallback_ratio`)
//!   is not armed (0 batches observed), its value is rendered as `-1` (never `0.0`),
//!   and `axon_b2_armed` is emitted as `0`.
//! - Single endpoint on Brain: aggregation covers Brain liveness, Indexer lifecycle
//!   heartbeat freshness, B2 VRAM pressure, B3 persist health, chunk queues, and
//!   per-project breakdown.

use crate::graph::GraphStore;
use serde_json::Value;

const HEARTBEAT_FRESHNESS_MS: i64 = 30_000;

fn parse_val_i64(v: Option<&Value>) -> i64 {
    v.and_then(|val| {
        val.as_i64()
            .or_else(|| val.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
    })
    .unwrap_or(0)
}

/// Project metric row parsed from `axon.project_telemetry`.
#[derive(Debug, Clone)]
pub struct ProjectMetricRow {
    pub project_code: String,
    pub files_total: i64,
    pub files_chunked: i64,
    pub symbols: i64,
    pub chunks_total: i64,
    pub chunks_embedded: i64,
    pub chunks_pending: i64,
    pub edges: i64,
    pub chunks_failed: i64,
    pub coverage_pct: f64,
}

/// Aggregated metrics snapshot.
#[derive(Debug, Clone)]
pub struct PrometheusMetricsSnapshot {
    pub brain_up: i64,
    pub indexer_alive: i64,
    pub indexer_heartbeat_age_seconds: f64,
    pub b2_armed: i64,
    pub b2_cpu_fallback_ratio: f64, // -1.0 if not armed
    pub b2_window_observed: i64,
    pub b2_window_cpu_fallbacks: i64,
    pub b2_session_recycles: i64,
    pub b2_resizes: i64,
    pub b2_gpu_batch_cap: i64,
    pub b2_degraded_threshold: f64,
    pub b2_critical_threshold: f64,
    pub b3_consecutive_failures: i64,
    pub b3_total_failures: i64,
    pub b3_total_successes: i64,
    pub b3_systemically_failing: i64,
    pub chunks_pending: i64,
    pub chunks_embedded: i64,
    pub chunks_total: i64,
    pub chunks_failed: i64,
    pub coverage_pct: f64,
    pub indexed_files_total: i64,
    pub indexed_files_chunked: i64,
    pub symbols_total: i64,
    pub edges_total: i64,
    pub projects: Vec<ProjectMetricRow>,
}

impl PrometheusMetricsSnapshot {
    pub fn collect(store: &GraphStore) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);

        let heartbeat = store.latest_lifecycle_heartbeat("indexer").ok().flatten();

        let (indexer_alive, heartbeat_age_seconds) = match heartbeat.as_ref() {
            Some(h) => {
                let age_ms = (now_ms - h.heartbeat_ms).max(0);
                let alive = if age_ms <= HEARTBEAT_FRESHNESS_MS { 1 } else { 0 };
                (alive, age_ms as f64 / 1000.0)
            }
            None => (0, -1.0),
        };

        let (
            b2_observed,
            b2_cpu_fallbacks,
            b2_session_recycles,
            b2_resizes,
            b2_gpu_batch_cap,
            b3_consecutive_failures,
            b3_total_failures,
            b3_total_successes,
        ) = match heartbeat.as_ref() {
            Some(h) => (
                h.b2_window_observed,
                h.b2_window_cpu_fallbacks,
                h.b2_session_recycles,
                h.b2_resizes,
                h.b2_gpu_batch_cap,
                h.b3_consecutive_failures,
                h.b3_total_failures,
                h.b3_total_successes,
            ),
            None => (0, 0, 0, 0, 0, 0, 0, 0),
        };

        // Strict invariant: when not armed, ratio is -1.0, NEVER 0.0
        let (b2_armed, b2_cpu_fallback_ratio) = if b2_observed > 0 {
            (1, b2_cpu_fallbacks as f64 / b2_observed as f64)
        } else {
            (0, -1.0)
        };

        let b3_systemically_failing = if b3_consecutive_failures >= 3 { 1 } else { 0 };

        // Read canonical project telemetry view
        let view_rows: Vec<Vec<Value>> = store
            .execute_raw_sql_gateway(
                "SELECT project_code, files_total, files_chunked, symbols, \
                        chunks_total, chunks_embedded, chunks_pending, edges, \
                        chunks_failed \
                 FROM axon.project_telemetry ORDER BY project_code ASC",
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();

        let mut projects = Vec::with_capacity(view_rows.len());
        let mut chunks_total = 0i64;
        let mut chunks_embedded = 0i64;
        let mut chunks_pending = 0i64;
        let mut chunks_failed = 0i64;
        let mut indexed_files_total = 0i64;
        let mut indexed_files_chunked = 0i64;
        let mut symbols_total = 0i64;
        let mut edges_total = 0i64;

        for row in view_rows {
            let project_code = row
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if project_code.is_empty() {
                continue;
            }
            let p_files_total = parse_val_i64(row.get(1));
            let p_files_chunked = parse_val_i64(row.get(2));
            let p_symbols = parse_val_i64(row.get(3));
            let p_chunks_total = parse_val_i64(row.get(4));
            let p_chunks_embedded = parse_val_i64(row.get(5));
            let p_chunks_pending = parse_val_i64(row.get(6));
            let p_edges = parse_val_i64(row.get(7));
            let p_chunks_failed = parse_val_i64(row.get(8));

            let p_cov = if p_chunks_total > 0 {
                ((p_chunks_embedded as f64 * 100.0) / p_chunks_total as f64).min(100.0)
            } else {
                0.0
            };

            chunks_total += p_chunks_total;
            chunks_embedded += p_chunks_embedded;
            chunks_pending += p_chunks_pending;
            chunks_failed += p_chunks_failed;
            indexed_files_total += p_files_total;
            indexed_files_chunked += p_files_chunked;
            symbols_total += p_symbols;
            edges_total += p_edges;

            projects.push(ProjectMetricRow {
                project_code,
                files_total: p_files_total,
                files_chunked: p_files_chunked,
                symbols: p_symbols,
                chunks_total: p_chunks_total,
                chunks_embedded: p_chunks_embedded,
                chunks_pending: p_chunks_pending,
                edges: p_edges,
                chunks_failed: p_chunks_failed,
                coverage_pct: p_cov,
            });
        }

        let coverage_pct = if chunks_total > 0 {
            ((chunks_embedded as f64 * 100.0) / chunks_total as f64).min(100.0)
        } else {
            0.0
        };

        Self {
            brain_up: 1,
            indexer_alive,
            indexer_heartbeat_age_seconds: heartbeat_age_seconds,
            b2_armed,
            b2_cpu_fallback_ratio,
            b2_window_observed: b2_observed,
            b2_window_cpu_fallbacks: b2_cpu_fallbacks,
            b2_session_recycles,
            b2_resizes,
            b2_gpu_batch_cap,
            b2_degraded_threshold: crate::pipeline::embed_pressure::DEGRADED_RATIO,
            b2_critical_threshold: crate::pipeline::embed_pressure::CRITICAL_RATIO,
            b3_consecutive_failures,
            b3_total_failures,
            b3_total_successes,
            b3_systemically_failing,
            chunks_pending,
            chunks_embedded,
            chunks_total,
            chunks_failed,
            coverage_pct,
            indexed_files_total,
            indexed_files_chunked,
            symbols_total,
            edges_total,
            projects,
        }
    }

    pub fn to_prometheus_text(&self) -> String {
        let mut out = String::with_capacity(4096);

        out.push_str("# HELP axon_brain_up Indicates whether the axon brain service is operational.\n");
        out.push_str("# TYPE axon_brain_up gauge\n");
        out.push_str(&format!("axon_brain_up {}\n\n", self.brain_up));

        out.push_str("# HELP up Standard Prometheus target availability gauge.\n");
        out.push_str("# TYPE up gauge\n");
        out.push_str(&format!("up {}\n\n", self.brain_up));

        out.push_str("# HELP axon_indexer_alive Indicates whether the indexer is considered alive based on recent lifecycle heartbeat (<= 30s).\n");
        out.push_str("# TYPE axon_indexer_alive gauge\n");
        out.push_str(&format!("axon_indexer_alive {}\n\n", self.indexer_alive));

        out.push_str("# HELP axon_indexer_heartbeat_age_seconds Age of the latest indexer lifecycle heartbeat in seconds (-1 if absent).\n");
        out.push_str("# TYPE axon_indexer_heartbeat_age_seconds gauge\n");
        out.push_str(&format!("axon_indexer_heartbeat_age_seconds {:.3}\n\n", self.indexer_heartbeat_age_seconds));

        out.push_str("# HELP axon_chunks_pending Total number of chunks pending embedding.\n");
        out.push_str("# TYPE axon_chunks_pending gauge\n");
        out.push_str(&format!("axon_chunks_pending {}\n\n", self.chunks_pending));

        out.push_str("# HELP axon_chunks_embedded Total number of embedded chunks.\n");
        out.push_str("# TYPE axon_chunks_embedded gauge\n");
        out.push_str(&format!("axon_chunks_embedded {}\n\n", self.chunks_embedded));

        out.push_str("# HELP axon_chunks_total Total number of chunks across all projects.\n");
        out.push_str("# TYPE axon_chunks_total gauge\n");
        out.push_str(&format!("axon_chunks_total {}\n\n", self.chunks_total));

        out.push_str("# HELP axon_chunks_failed Total number of failed chunks (terminal).\n");
        out.push_str("# TYPE axon_chunks_failed gauge\n");
        out.push_str(&format!("axon_chunks_failed {}\n\n", self.chunks_failed));

        out.push_str("# HELP axon_coverage_pct Percentage of chunks embedded (0.00 to 100.00).\n");
        out.push_str("# TYPE axon_coverage_pct gauge\n");
        out.push_str(&format!("axon_coverage_pct {:.2}\n\n", self.coverage_pct));

        out.push_str("# HELP axon_b2_armed Whether B2 VRAM pressure monitoring is armed (1) or not_armed (0).\n");
        out.push_str("# TYPE axon_b2_armed gauge\n");
        out.push_str(&format!("axon_b2_armed {}\n\n", self.b2_armed));

        out.push_str("# HELP axon_b2_cpu_fallback_ratio Ratio of B2 batches falling back to CPU due to VRAM pressure (-1 if not armed).\n");
        out.push_str("# TYPE axon_b2_cpu_fallback_ratio gauge\n");
        if self.b2_armed == 0 {
            out.push_str("axon_b2_cpu_fallback_ratio -1\n\n");
        } else {
            out.push_str(&format!("axon_b2_cpu_fallback_ratio {:.4}\n\n", self.b2_cpu_fallback_ratio));
        }

        out.push_str("# HELP axon_b2_window_observed Number of batches observed in B2 rolling window.\n");
        out.push_str("# TYPE axon_b2_window_observed gauge\n");
        out.push_str(&format!("axon_b2_window_observed {}\n\n", self.b2_window_observed));

        out.push_str("# HELP axon_b2_window_cpu_fallbacks Number of CPU fallback batches in B2 rolling window.\n");
        out.push_str("# TYPE axon_b2_window_cpu_fallbacks gauge\n");
        out.push_str(&format!("axon_b2_window_cpu_fallbacks {}\n\n", self.b2_window_cpu_fallbacks));

        out.push_str("# HELP axon_b2_session_recycles Number of ORT session recycles due to VRAM pressure.\n");
        out.push_str("# TYPE axon_b2_session_recycles gauge\n");
        out.push_str(&format!("axon_b2_session_recycles {}\n\n", self.b2_session_recycles));

        out.push_str("# HELP axon_b2_resizes Number of GPU batch size shrink resizes.\n");
        out.push_str("# TYPE axon_b2_resizes gauge\n");
        out.push_str(&format!("axon_b2_resizes {}\n\n", self.b2_resizes));

        out.push_str("# HELP axon_b2_gpu_batch_cap Current cap on GPU batch size (0 if default).\n");
        out.push_str("# TYPE axon_b2_gpu_batch_cap gauge\n");
        out.push_str(&format!("axon_b2_gpu_batch_cap {}\n\n", self.b2_gpu_batch_cap));

        out.push_str("# HELP axon_b2_degraded_threshold Degraded threshold for B2 CPU fallback ratio.\n");
        out.push_str("# TYPE axon_b2_degraded_threshold gauge\n");
        out.push_str(&format!("axon_b2_degraded_threshold {:.2}\n\n", self.b2_degraded_threshold));

        out.push_str("# HELP axon_b2_critical_threshold Critical threshold for B2 CPU fallback ratio.\n");
        out.push_str("# TYPE axon_b2_critical_threshold gauge\n");
        out.push_str(&format!("axon_b2_critical_threshold {:.2}\n\n", self.b2_critical_threshold));

        out.push_str("# HELP axon_b3_consecutive_failures Consecutive persist failures on stage B3.\n");
        out.push_str("# TYPE axon_b3_consecutive_failures gauge\n");
        out.push_str(&format!("axon_b3_consecutive_failures {}\n\n", self.b3_consecutive_failures));

        out.push_str("# HELP axon_b3_total_failures Total persist failures on stage B3.\n");
        out.push_str("# TYPE axon_b3_total_failures counter\n");
        out.push_str(&format!("axon_b3_total_failures {}\n\n", self.b3_total_failures));

        out.push_str("# HELP axon_b3_total_successes Total persist successes on stage B3.\n");
        out.push_str("# TYPE axon_b3_total_successes counter\n");
        out.push_str(&format!("axon_b3_total_successes {}\n\n", self.b3_total_successes));

        out.push_str("# HELP axon_b3_systemically_failing Systemic failure flag for stage B3 (1 if consecutive failures >= 3).\n");
        out.push_str("# TYPE axon_b3_systemically_failing gauge\n");
        out.push_str(&format!("axon_b3_systemically_failing {}\n\n", self.b3_systemically_failing));

        out.push_str("# HELP axon_indexed_files_total Total enrolled files in IndexedFile across all projects.\n");
        out.push_str("# TYPE axon_indexed_files_total gauge\n");
        out.push_str(&format!("axon_indexed_files_total {}\n\n", self.indexed_files_total));

        out.push_str("# HELP axon_indexed_files_chunked Total files with at least one chunk.\n");
        out.push_str("# TYPE axon_indexed_files_chunked gauge\n");
        out.push_str(&format!("axon_indexed_files_chunked {}\n\n", self.indexed_files_chunked));

        out.push_str("# HELP axon_symbols_total Total indexed symbols across all projects.\n");
        out.push_str("# TYPE axon_symbols_total gauge\n");
        out.push_str(&format!("axon_symbols_total {}\n\n", self.symbols_total));

        out.push_str("# HELP axon_edges_total Total indexed edges across all projects.\n");
        out.push_str("# TYPE axon_edges_total gauge\n");
        out.push_str(&format!("axon_edges_total {}\n\n", self.edges_total));

        if !self.projects.is_empty() {
            out.push_str("# HELP axon_project_chunks_pending Pending chunks per project.\n");
            out.push_str("# TYPE axon_project_chunks_pending gauge\n");
            for p in &self.projects {
                out.push_str(&format!("axon_project_chunks_pending{{project=\"{}\"}} {}\n", p.project_code, p.chunks_pending));
            }
            out.push('\n');

            out.push_str("# HELP axon_project_chunks_embedded Embedded chunks per project.\n");
            out.push_str("# TYPE axon_project_chunks_embedded gauge\n");
            for p in &self.projects {
                out.push_str(&format!("axon_project_chunks_embedded{{project=\"{}\"}} {}\n", p.project_code, p.chunks_embedded));
            }
            out.push('\n');

            out.push_str("# HELP axon_project_chunks_total Total chunks per project.\n");
            out.push_str("# TYPE axon_project_chunks_total gauge\n");
            for p in &self.projects {
                out.push_str(&format!("axon_project_chunks_total{{project=\"{}\"}} {}\n", p.project_code, p.chunks_total));
            }
            out.push('\n');

            out.push_str("# HELP axon_project_coverage_pct Chunk embedding coverage percentage per project.\n");
            out.push_str("# TYPE axon_project_coverage_pct gauge\n");
            for p in &self.projects {
                out.push_str(&format!("axon_project_coverage_pct{{project=\"{}\"}} {:.2}\n", p.project_code, p.coverage_pct));
            }
            out.push('\n');

            out.push_str("# HELP axon_project_files_total Total enrolled files per project.\n");
            out.push_str("# TYPE axon_project_files_total gauge\n");
            for p in &self.projects {
                out.push_str(&format!("axon_project_files_total{{project=\"{}\"}} {}\n", p.project_code, p.files_total));
            }
            out.push('\n');

            out.push_str("# HELP axon_project_files_chunked Chunked files per project.\n");
            out.push_str("# TYPE axon_project_files_chunked gauge\n");
            for p in &self.projects {
                out.push_str(&format!("axon_project_files_chunked{{project=\"{}\"}} {}\n", p.project_code, p.files_chunked));
            }
            out.push('\n');
        }

        out
    }
}

pub fn render_prometheus_metrics(store: &GraphStore) -> String {
    let snapshot = PrometheusMetricsSnapshot::collect(store);
    snapshot.to_prometheus_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unarmed_b2_ratio_strict_minus_one() {
        let snap = PrometheusMetricsSnapshot {
            brain_up: 1,
            indexer_alive: 0,
            indexer_heartbeat_age_seconds: -1.0,
            b2_armed: 0,
            b2_cpu_fallback_ratio: -1.0,
            b2_window_observed: 0,
            b2_window_cpu_fallbacks: 0,
            b2_session_recycles: 0,
            b2_resizes: 0,
            b2_gpu_batch_cap: 0,
            b2_degraded_threshold: 0.05,
            b2_critical_threshold: 0.20,
            b3_consecutive_failures: 0,
            b3_total_failures: 0,
            b3_total_successes: 0,
            b3_systemically_failing: 0,
            chunks_pending: 10,
            chunks_embedded: 90,
            chunks_total: 100,
            chunks_failed: 0,
            coverage_pct: 90.0,
            indexed_files_total: 5,
            indexed_files_chunked: 5,
            symbols_total: 20,
            edges_total: 30,
            projects: vec![ProjectMetricRow {
                project_code: "TEST".to_string(),
                files_total: 5,
                files_chunked: 5,
                symbols: 20,
                chunks_total: 100,
                chunks_embedded: 90,
                chunks_pending: 10,
                edges: 30,
                chunks_failed: 0,
                coverage_pct: 90.0,
            }],
        };

        let text = snap.to_prometheus_text();
        assert!(text.contains("axon_b2_armed 0"));
        assert!(text.contains("axon_b2_cpu_fallback_ratio -1"));
        assert!(!text.contains("axon_b2_cpu_fallback_ratio 0"));
        assert!(text.contains("axon_chunks_pending 10"));
        assert!(text.contains("axon_chunks_embedded 90"));
        assert!(text.contains("axon_chunks_total 100"));
        assert!(text.contains("axon_coverage_pct 90.00"));
        assert!(text.contains("axon_project_chunks_pending{project=\"TEST\"} 10"));
        assert!(text.contains("axon_project_coverage_pct{project=\"TEST\"} 90.00"));
    }

    #[test]
    fn test_armed_b2_ratio_rendered_exact() {
        let snap = PrometheusMetricsSnapshot {
            brain_up: 1,
            indexer_alive: 1,
            indexer_heartbeat_age_seconds: 4.5,
            b2_armed: 1,
            b2_cpu_fallback_ratio: 0.1250,
            b2_window_observed: 8,
            b2_window_cpu_fallbacks: 1,
            b2_session_recycles: 2,
            b2_resizes: 1,
            b2_gpu_batch_cap: 32,
            b2_degraded_threshold: 0.05,
            b2_critical_threshold: 0.20,
            b3_consecutive_failures: 0,
            b3_total_failures: 0,
            b3_total_successes: 10,
            b3_systemically_failing: 0,
            chunks_pending: 0,
            chunks_embedded: 100,
            chunks_total: 100,
            chunks_failed: 0,
            coverage_pct: 100.0,
            indexed_files_total: 10,
            indexed_files_chunked: 10,
            symbols_total: 50,
            edges_total: 80,
            projects: vec![],
        };

        let text = snap.to_prometheus_text();
        assert!(text.contains("axon_b2_armed 1"));
        assert!(text.contains("axon_b2_cpu_fallback_ratio 0.1250"));
        assert!(text.contains("axon_b2_window_observed 8"));
        assert!(text.contains("axon_b2_window_cpu_fallbacks 1"));
        assert!(text.contains("axon_b2_session_recycles 2"));
    }
}
