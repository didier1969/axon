//! Streaming pipeline v2 — REQ-AXO-289 / CPT-AXO-054.
//!
//! Six independent stages over two pipelines (A graph, B vector), each backed by
//! a per-stage worker pool draining a bounded `tokio::sync::mpsc` channel.
//!
//! This is the scaffolding module: it defines [`StageMetrics`], the
//! [`spawn_stage_workers`] helper, and the canonical channel capacities used
//! across the pipeline. Concrete stage implementations (A1/A2/A3, B1/B2/B3) are
//! wired in slices S2–S5 of REQ-AXO-289.

pub mod channels;
pub mod indexed_file_cache;
pub mod metrics;
pub mod stage_a1;
pub mod types;
pub mod worker_pool;

pub use channels::{
    PipelineChannelCaps, A3_TO_B1_BUFFER_CAP_DEFAULT, B1_COLDSTART_BATCH_SIZE_DEFAULT,
    INTERNAL_CHANNEL_CAP_DEFAULT,
};
pub use indexed_file_cache::{IndexedFileCache, IndexedFileEntry};
pub use metrics::{StageMetrics, StageSnapshot};
pub use stage_a1::a1_prepare;
pub use types::PreparedFile;
pub use worker_pool::spawn_stage_workers;
