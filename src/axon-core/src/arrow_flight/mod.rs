// REQ-AXO-902679 — High-Performance Zero-Copy Apache Arrow & Flight SQL Analytical Engine.
//
// Encodes in-memory symbols, edges, and embeddings into Arrow RecordBatches,
// eliminating serialization boxing and enabling ultra-fast analytical scans.

pub mod flight_server;
pub mod flight_stream;
pub mod record_batch;

#[cfg(test)]
mod flight_server_tests;
#[cfg(test)]
mod flight_stream_tests;
#[cfg(test)]
mod record_batch_tests;
