// REQ-AXO-902679 — High-Performance Arrow Flight SQL Streaming Bridge.
//
// Streams large tabular datasets of symbols and graph edges as bounded
// Arrow RecordBatches, eliminating OOM spikes on large analytic queries.

use std::pin::Pin;

use anyhow::Result;
use arrow_array::RecordBatch;
use futures_util::stream::{self, Stream, StreamExt};

use crate::arrow_flight::record_batch::{edges_to_record_batch, symbols_to_record_batch};
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeRecord};

/// Stream bridge producing sequences of Arrow RecordBatches.
pub struct FlightSqlStreamBridge;

impl FlightSqlStreamBridge {
    pub fn new() -> Self {
        Self
    }

    /// Streams symbols as chunked RecordBatches of maximum `batch_size` rows.
    pub fn stream_symbols_batches(
        &self,
        nodes: Vec<NodeRecord>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send>> {
        let chunk_size = batch_size.max(1);
        let chunks: Vec<Vec<NodeRecord>> = nodes.chunks(chunk_size).map(|c| c.to_vec()).collect();

        let s = stream::iter(chunks).map(|chunk| symbols_to_record_batch(&chunk));
        Box::pin(s)
    }

    /// Streams edges as chunked RecordBatches of maximum `batch_size` rows.
    pub fn stream_edges_batches(
        &self,
        edges: Vec<EdgeTriple>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send>> {
        let chunk_size = batch_size.max(1);
        let chunks: Vec<Vec<EdgeTriple>> = edges.chunks(chunk_size).map(|c| c.to_vec()).collect();

        let s = stream::iter(chunks).map(|chunk| edges_to_record_batch(&chunk));
        Box::pin(s)
    }
}

impl Default for FlightSqlStreamBridge {
    fn default() -> Self {
        Self::new()
    }
}
