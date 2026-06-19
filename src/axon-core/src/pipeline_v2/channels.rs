//! Channel topology constants for the streaming pipeline (CPT-AXO-054).
//!
//! These are operator-overridable through env vars (see REQ-AXO-290). The
//! defaults wired here match the architecture decisions made during the
//! session 17 design conversation (2026-05-12):
//!
//! * Internal channels A1→A2, A2→A3, B2→B3, b_chunks (demand_pull→B2)
//!   — capacity 1024 each; absorbs ~1 second of burst latency variance.
//! * Slice 5 SOTA — cross-pipeline channel A3→B1 supprimé. B est nourri
//!   exclusivement via demand_pull_b (PG NOTIFY `chunk_pending_embed`
//!   wake + SELECT-with-content), single-source sans silent-drop buffer.

/// Default capacity for the bounded channels that connect adjacent stages
/// *within* the same pipeline (A1→A2, A2→A3, B2→B3) AND the b_chunks
/// channel (demand_pull → B2).
pub const INTERNAL_CHANNEL_CAP_DEFAULT: usize = 1024;

/// REQ-AXO-901906 / REQ-AXO-902038 — capacity for the pipeline-A channels
/// that carry file CONTENT (`A1→A2`, `A2→A3` hold a `PreparedFile`/
/// `ParsedFile` with up to `max_parse_bytes` ≈ 5 MB each). This is the
/// canonical pipeline-A memory bound: `a_content × max_parse_bytes × 2
/// channels`. Paired with `send().await` backpressure (channel-as-buffer).
///
/// REQ-AXO-902038 (session 84) — the old raw-count default of 256 bounded
/// worst-case A buffering at `256 × 5 MB × 2 ≈ 2.5 GB`, which thrashed a
/// host co-running the live instance under a slow/wedged B drain (PIL-AXO-007
/// host-safety / PIL-AXO-004 cohabitation violation). The effective default
/// is now DERIVED from a host-safe memory budget (see
/// [`A_CONTENT_MEMORY_BUDGET_MB_DEFAULT`] + [`derive_a_content_cap`]) so the
/// bound tracks `max_parse_bytes` instead of a blind file count. This const
/// is the static fallback used by `Default` (no env) — itself sized to keep
/// the worst case ≈ 640 MB. Override the count directly via
/// `AXON_PIPELINE_A_CONTENT_CAP`, or the budget via
/// `AXON_PIPELINE_A_MEMORY_BUDGET_MB`.
pub const A_CONTENT_CHANNEL_CAP_DEFAULT: usize = 64;

/// REQ-AXO-902038 — host-safe memory budget (MiB) for the two pipeline-A
/// content channels combined. The effective `a_content` capacity is derived
/// as `budget / (max_parse_bytes × 2)` so the worst-case A buffering stays
/// bounded regardless of file sizes, on a host shared with the live
/// instance. 768 MiB keeps the bound well under 1 GB. Operators on a
/// dedicated indexing host can raise it via `AXON_PIPELINE_A_MEMORY_BUDGET_MB`
/// for more A↔A pipelining depth. Override: `AXON_PIPELINE_A_MEMORY_BUDGET_MB`.
pub const A_CONTENT_MEMORY_BUDGET_MB_DEFAULT: u64 = 768;

/// REQ-AXO-902038 — derive the host-safe `a_content` channel capacity from a
/// total memory `budget_mb` and the per-file `max_parse_bytes` ceiling.
/// Bound = `a_content × max_parse_bytes × 2 channels ≤ budget`. Clamped to a
/// sane pipelining range so a tiny budget still pipelines and a huge one does
/// not over-buffer. Pure (no env) → unit-testable.
pub fn derive_a_content_cap(budget_mb: u64, max_parse_bytes: u64) -> usize {
    let budget_bytes = budget_mb.saturating_mul(1024 * 1024);
    let per_slot_bytes = max_parse_bytes.max(1).saturating_mul(2); // two channels
    let derived = (budget_bytes / per_slot_bytes) as usize;
    derived.clamp(8, 512)
}

/// REQ-AXO-289 S4b'/REQ-AXO-262 — Default batch size for the B2 GPU
/// embedder. ORT/TensorRT BGE-Large hits its peak throughput around
/// batch=64-128. At batch=1 the GPU is essentially idle (~10 ch/s vs
/// ~280 ch/s peak). The B2 worker accumulates up to this many chunks
/// per `embed_batch` call before flushing to the GPU. Operator
/// override: `AXON_B2_BATCH_SIZE`.
pub const B2_BATCH_SIZE_DEFAULT: usize = 64;

/// REQ-AXO-289 S4b' — Maximum time the B2 worker waits before
/// flushing a partial batch. Bounds latency under low-traffic regimes
/// (cold start, post-pause warmup, end-of-walk tail). Operator
/// override: `AXON_B2_BATCH_TIMEOUT_MS`.
pub const B2_BATCH_TIMEOUT_MS_DEFAULT: u64 = 200;

/// REQ-AXO-295 — Default batch size for the A3 PG writer. A3 per-file
/// transactions saturate at single-digit concurrent workers because
/// every file is a `BEGIN/INSERT…/COMMIT` round-trip on the same DB,
/// so adding workers thrashes pg_locks rather than scaling throughput
/// (measured 2026-05-12: A3=2 → 57 ch/s, A3=6 → 22 ch/s in NoOp). The
/// batched worker accumulates N parsed files and writes them all in
/// a single `execute_batch`, amortizing transaction overhead.
/// Operator override: `AXON_A3_BATCH_SIZE`.
pub const A3_BATCH_SIZE_DEFAULT: usize = 32;

/// REQ-AXO-295 — Maximum time the A3 worker waits before flushing a
/// partial batch. Operator-requested 10 ms floor (2026-05-12):
/// "envoyer ce qu'on a toutes les 10 ms, jamais en dessous". Operator
/// override: `AXON_A3_BATCH_TIMEOUT_MS`.
pub const A3_BATCH_TIMEOUT_MS_DEFAULT: u64 = 10;

/// Default batch size for the B3 ChunkEmbedding UPSERT writer.
/// Multi-row UPSERTs amortize pgvector HNSW index maintenance cost
/// (a single transaction does N graph mutations vs N transactions ×
/// 1 mutation). 256 = 4× B2's 64-batch — B2 flushes faster than B3
/// can drain at single-batch granularity, so widening B3 closes the
/// downstream throttle. Operator override: `AXON_B3_BATCH_SIZE`.
pub const B3_BATCH_SIZE_DEFAULT: usize = 256;

/// REQ-AXO-901678 — default batch size for the pipeline_v2 runtime drain
/// loop in `pipeline_v2_runtime::spawn_pipeline_v2_indexer`. Replaces the
/// legacy hardcoded 256 cap that saturated under multi-project cold
/// starts (session-54 bench : 338-file scan triggered repeated
/// `last_batch_dropped_full=256` heartbeats, cumulative ~2.7k drops in
/// 60s while A3 ran at work_ratio=0.99). Bumping to 512 doubles the
/// drain bandwidth without inflating per-tick lock-hold time
/// observably ; operator override : `AXON_INGRESS_DRAIN_BATCH`.
pub const INGRESS_DRAIN_BATCH_DEFAULT: usize = 512;

/// B3 partial-batch flush timeout. **Critical: 200 ms, not 10 ms.**
/// Prior 10 ms default was copy-pasted from A3 (whose 10 ms floor
/// was operator-requested 2026-05-12 for FTS visibility latency).
/// B3 is the terminal vector writer — embedding latency adds nothing
/// downstream, while a too-eager flush degrades the effective batch
/// to ~1 row per tick under realistic B2 arrival rates (100-300/s),
/// nullifying `B3_BATCH_SIZE`. 200 ms gives B3 enough wall time to
/// collect a full batch from B2's GPU bursts. Operator override:
/// `AXON_B3_BATCH_TIMEOUT_MS`.
pub const B3_BATCH_TIMEOUT_MS_DEFAULT: u64 = 200;

/// Effective pipeline channel capacities after env-var resolution.
///
/// Use [`PipelineChannelCaps::from_env`] to derive a single owned value at
/// boot and pass it into the wiring code.
#[derive(Debug, Clone, Copy)]
pub struct PipelineChannelCaps {
    pub internal: usize,
    /// Capacity of the A content-carrying channels (A1→A2, A2→A3). The
    /// pipeline-A memory bound (REQ-AXO-901906).
    pub a_content: usize,
    pub a3_batch_size: usize,
    pub a3_batch_timeout_ms: u64,
    pub b2_batch_size: usize,
    pub b2_batch_timeout_ms: u64,
    pub b3_batch_size: usize,
    pub b3_batch_timeout_ms: u64,
    pub ingress_drain_batch: usize,
}

impl Default for PipelineChannelCaps {
    fn default() -> Self {
        Self {
            internal: INTERNAL_CHANNEL_CAP_DEFAULT,
            a_content: A_CONTENT_CHANNEL_CAP_DEFAULT,
            a3_batch_size: A3_BATCH_SIZE_DEFAULT,
            a3_batch_timeout_ms: A3_BATCH_TIMEOUT_MS_DEFAULT,
            b2_batch_size: B2_BATCH_SIZE_DEFAULT,
            b2_batch_timeout_ms: B2_BATCH_TIMEOUT_MS_DEFAULT,
            b3_batch_size: B3_BATCH_SIZE_DEFAULT,
            b3_batch_timeout_ms: B3_BATCH_TIMEOUT_MS_DEFAULT,
            ingress_drain_batch: INGRESS_DRAIN_BATCH_DEFAULT,
        }
    }
}

impl PipelineChannelCaps {
    /// Read capacities from env vars (REQ-AXO-290), falling back to defaults
    /// when unset or unparsable.
    pub fn from_env() -> Self {
        let mut caps = Self::default();
        if let Ok(raw) = std::env::var("AXON_PIPELINE_INTERNAL_CHANNEL_CAP") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.internal = parsed;
                }
            }
        }
        // REQ-AXO-902038 — host-safe A content-channel sizing. An explicit
        // count override wins; otherwise DERIVE the cap from a memory budget
        // so the worst-case A buffering (`a_content × max_parse_bytes × 2`)
        // stays host-safe regardless of file sizes (the 2.5 GB thrash, s84).
        if let Some(explicit) = std::env::var("AXON_PIPELINE_A_CONTENT_CAP")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .filter(|&v| v > 0)
        {
            caps.a_content = explicit;
        } else {
            let budget_mb = std::env::var("AXON_PIPELINE_A_MEMORY_BUDGET_MB")
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|&v| v > 0)
                .unwrap_or(A_CONTENT_MEMORY_BUDGET_MB_DEFAULT);
            caps.a_content =
                derive_a_content_cap(budget_mb, crate::indexing_policy::max_parse_bytes());
        }
        if let Ok(raw) = std::env::var("AXON_B2_BATCH_SIZE") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.b2_batch_size = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_B2_BATCH_TIMEOUT_MS") {
            if let Ok(parsed) = raw.trim().parse::<u64>() {
                if parsed > 0 {
                    caps.b2_batch_timeout_ms = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_A3_BATCH_SIZE") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.a3_batch_size = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_A3_BATCH_TIMEOUT_MS") {
            if let Ok(parsed) = raw.trim().parse::<u64>() {
                if parsed > 0 {
                    caps.a3_batch_timeout_ms = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_B3_BATCH_SIZE") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.b3_batch_size = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_B3_BATCH_TIMEOUT_MS") {
            if let Ok(parsed) = raw.trim().parse::<u64>() {
                if parsed > 0 {
                    caps.b3_batch_timeout_ms = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_INGRESS_DRAIN_BATCH") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.ingress_drain_batch = parsed;
                }
            }
        }
        caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_a_content_cap_keeps_worst_case_under_budget() {
        // REQ-AXO-902038 — default 768 MiB budget, 5 MiB max parse → the
        // worst-case A buffering (cap × 5 MiB × 2 channels) must stay under
        // the budget, and well under the 2.5 GB that thrashed the host.
        let five_mib = 5 * 1024 * 1024;
        let cap = derive_a_content_cap(A_CONTENT_MEMORY_BUDGET_MB_DEFAULT, five_mib);
        let worst_case_bytes = (cap as u64) * five_mib * 2;
        let budget_bytes = A_CONTENT_MEMORY_BUDGET_MB_DEFAULT * 1024 * 1024;
        assert!(
            worst_case_bytes <= budget_bytes,
            "worst case {worst_case_bytes} must be <= budget {budget_bytes} (cap={cap})"
        );
        assert!(
            worst_case_bytes < 1024 * 1024 * 1024,
            "worst case must stay under 1 GiB, got {worst_case_bytes} (cap={cap})"
        );
        // Sanity: still deep enough to pipeline (not collapsed to the floor).
        assert!(cap >= 32, "cap should still allow real pipelining, got {cap}");
    }

    #[test]
    fn derive_a_content_cap_tracks_max_parse_bytes() {
        // A smaller per-file ceiling permits a deeper channel for the same
        // memory budget (the bound is precise w.r.t. max_parse_bytes, not a
        // blind file count).
        let small = derive_a_content_cap(768, 1024 * 1024); // 1 MiB files
        let large = derive_a_content_cap(768, 5 * 1024 * 1024); // 5 MiB files
        assert!(
            small > large,
            "smaller files → deeper cap (small={small}, large={large})"
        );
    }

    #[test]
    fn derive_a_content_cap_is_clamped() {
        // Tiny budget still pipelines (floor), huge budget does not unbound.
        assert_eq!(derive_a_content_cap(1, 100 * 1024 * 1024), 8); // floor
        assert_eq!(derive_a_content_cap(1_000_000, 1024), 512); // ceiling
    }

    #[test]
    fn default_a_content_cap_is_host_safe() {
        // The static Default (no env) must keep the worst case well bounded
        // (≈ 640 MiB at 5 MiB/file), never the legacy 2.5 GB.
        let worst = (A_CONTENT_CHANNEL_CAP_DEFAULT as u64) * 5 * 1024 * 1024 * 2;
        assert!(
            worst <= 768 * 1024 * 1024,
            "default worst case {worst} must be host-safe"
        );
    }
}
