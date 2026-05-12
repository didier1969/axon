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
pub const B1_COLDSTART_BATCH_SIZE_DEFAULT: usize = 256;

/// Effective pipeline channel capacities after env-var resolution.
///
/// Use [`PipelineChannelCaps::from_env`] to derive a single owned value at
/// boot and pass it into the wiring code.
#[derive(Debug, Clone, Copy)]
pub struct PipelineChannelCaps {
    pub internal: usize,
    pub a3_to_b1: usize,
    pub b1_coldstart_batch_size: usize,
}

impl Default for PipelineChannelCaps {
    fn default() -> Self {
        Self {
            internal: INTERNAL_CHANNEL_CAP_DEFAULT,
            a3_to_b1: A3_TO_B1_BUFFER_CAP_DEFAULT,
            b1_coldstart_batch_size: B1_COLDSTART_BATCH_SIZE_DEFAULT,
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
        caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_canonical_session_17_decisions() {
        let caps = PipelineChannelCaps::default();
        assert_eq!(caps.internal, 1024);
        assert_eq!(caps.a3_to_b1, 10_000);
        assert_eq!(caps.b1_coldstart_batch_size, 256);
    }
}
