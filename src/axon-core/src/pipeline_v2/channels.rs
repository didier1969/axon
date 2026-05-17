//! Channel topology constants for the streaming pipeline (CPT-AXO-054).
//!
//! These are operator-overridable through env vars (see REQ-AXO-290). The
//! defaults wired here match the architecture decisions made during the
//! session 17 design conversation (2026-05-12):
//!
//! * Internal channels A1→A2, A2→A3, B1→B2, B2→B3 — capacity 1024 each; this
//!   absorbs ~1 second of burst latency variance between adjacent stages at
//!   the typical throughput envelope.
//! * Cross-pipeline channel A3→B1 — capacity 10 000; this is the only buffer
//!   that has to absorb GPU-vs-CPU pace asymmetry. A3 publishes via
//!   `try_send` (non-blocking) so the graph pipeline never stalls on B's
//!   pace; B compensates via the cold-start poll DB pathway.
//! * Cold-start poll batch size — 256 chunk IDs per `SELECT` rattrapage round.

/// Default capacity for the bounded channels that connect adjacent stages
/// *within* the same pipeline (A1→A2, A2→A3, B1→B2, B2→B3).
pub const INTERNAL_CHANNEL_CAP_DEFAULT: usize = 1024;

/// Default capacity for the cross-pipeline buffer between A3 and B1.
///
/// Sized to absorb roughly 20–30 s of graph-pipeline burst at peak throughput
/// when the GPU is the slower side. Operator override:
/// `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP`.
pub const A3_TO_B1_BUFFER_CAP_DEFAULT: usize = 10_000;

/// Default `LIMIT N` for the B1 cold-start poll query.
///
/// `SELECT chunk_id FROM Chunk LEFT JOIN ChunkEmbedding ce ON ce.chunk_id =
/// Chunk.id WHERE ce.chunk_id IS NULL LIMIT N` — run once at indexer startup
/// to enqueue every chunk that was graphed but never vectorised. Operator
/// override: `AXON_B1_COLDSTART_BATCH_SIZE`.
pub const B1_COLDSTART_BATCH_SIZE_DEFAULT: usize = 4096;

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
    pub a3_to_b1: usize,
    pub b1_coldstart_batch_size: usize,
    pub a3_batch_size: usize,
    pub a3_batch_timeout_ms: u64,
    pub b2_batch_size: usize,
    pub b2_batch_timeout_ms: u64,
    pub b3_batch_size: usize,
    pub b3_batch_timeout_ms: u64,
}

impl Default for PipelineChannelCaps {
    fn default() -> Self {
        Self {
            internal: INTERNAL_CHANNEL_CAP_DEFAULT,
            a3_to_b1: A3_TO_B1_BUFFER_CAP_DEFAULT,
            b1_coldstart_batch_size: B1_COLDSTART_BATCH_SIZE_DEFAULT,
            a3_batch_size: A3_BATCH_SIZE_DEFAULT,
            a3_batch_timeout_ms: A3_BATCH_TIMEOUT_MS_DEFAULT,
            b2_batch_size: B2_BATCH_SIZE_DEFAULT,
            b2_batch_timeout_ms: B2_BATCH_TIMEOUT_MS_DEFAULT,
            b3_batch_size: B3_BATCH_SIZE_DEFAULT,
            b3_batch_timeout_ms: B3_BATCH_TIMEOUT_MS_DEFAULT,
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
        if let Ok(raw) = std::env::var("AXON_PIPELINE_A3_TO_B1_BUFFER_CAP") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.a3_to_b1 = parsed;
                }
            }
        }
        if let Ok(raw) = std::env::var("AXON_B1_COLDSTART_BATCH_SIZE") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                if parsed > 0 {
                    caps.b1_coldstart_batch_size = parsed;
                }
            }
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
        caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_canonical_session_19_decisions() {
        // REQ-AXO-91567 — `B1_COLDSTART_BATCH_SIZE_DEFAULT` was bumped
        // from 256 (original session-19 figure) to 4096 to absorb
        // larger boot-time backlogs in one query. Test value updated
        // in step.
        let caps = PipelineChannelCaps::default();
        assert_eq!(caps.internal, 1024);
        assert_eq!(caps.a3_to_b1, 10_000);
        assert_eq!(caps.b1_coldstart_batch_size, 4096);
        assert_eq!(caps.b2_batch_size, 64);
        assert_eq!(caps.b2_batch_timeout_ms, 200);
    }

    #[test]
    fn defaults_match_req_axo_295_batching_decisions() {
        let caps = PipelineChannelCaps::default();
        assert_eq!(caps.a3_batch_size, 32);
        assert_eq!(caps.a3_batch_timeout_ms, 10);
        assert_eq!(caps.b3_batch_size, 256);
        assert_eq!(caps.b3_batch_timeout_ms, 200);
    }
}
