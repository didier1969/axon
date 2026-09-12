// REQ-AXO-902679 — High-Performance Zero-Copy Apache Arrow RecordBatch Encoders.
//
// Converts in-memory IST nodes, edges, and embeddings into vectorized Arrow
// RecordBatches without JSON boxing or intermediate string marshalling.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    ArrayRef, FixedSizeListArray, Float32Array, Int32Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use once_cell::sync::Lazy;

use crate::ist_snapshot::snapshot::{EdgeTriple, NodeRecord};

pub static SYMBOLS_SCHEMA: Lazy<SchemaRef> = Lazy::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("project", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("complexity", DataType::Int32, false),
    ]))
});

pub static EDGES_SCHEMA: Lazy<SchemaRef> = Lazy::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("source", DataType::Utf8, false),
        Field::new("target", DataType::Utf8, false),
        Field::new("relation_type", DataType::Utf8, false),
    ]))
});

pub fn embeddings_schema(dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
            false,
        ),
    ]))
}

/// Vectorized batch conversion of IST symbol node records into an Arrow RecordBatch.
pub fn symbols_to_record_batch(nodes: &[NodeRecord]) -> Result<RecordBatch> {
    let mut ids = Vec::with_capacity(nodes.len());
    let mut names = Vec::with_capacity(nodes.len());
    let mut projects = Vec::with_capacity(nodes.len());
    let mut kinds = Vec::with_capacity(nodes.len());
    let mut complexities = Vec::with_capacity(nodes.len());

    for node in nodes {
        ids.push(node.id.as_str());
        names.push(node.name.as_str());
        projects.push(node.project_code.as_str());
        kinds.push(node.kind.as_db());
        complexities.push(node.complexity.unwrap_or(-1));
    }

    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(ids)),
        Arc::new(StringArray::from(names)),
        Arc::new(StringArray::from(projects)),
        Arc::new(StringArray::from(kinds)),
        Arc::new(Int32Array::from(complexities)),
    ];

    RecordBatch::try_new(Arc::clone(&SYMBOLS_SCHEMA), cols)
        .context("failed to construct symbols RecordBatch")
}

/// Vectorized batch conversion of IST edge triples into an Arrow RecordBatch.
pub fn edges_to_record_batch(edges: &[EdgeTriple]) -> Result<RecordBatch> {
    let mut sources = Vec::with_capacity(edges.len());
    let mut targets = Vec::with_capacity(edges.len());
    let mut rels = Vec::with_capacity(edges.len());

    for edge in edges {
        sources.push(edge.source.as_str());
        targets.push(edge.target.as_str());
        rels.push(edge.rel.as_db());
    }

    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(sources)),
        Arc::new(StringArray::from(targets)),
        Arc::new(StringArray::from(rels)),
    ];

    RecordBatch::try_new(Arc::clone(&EDGES_SCHEMA), cols)
        .context("failed to construct edges RecordBatch")
}

/// Vectorized conversion of chunk embeddings into a FixedSizeList Arrow RecordBatch.
pub fn embeddings_to_record_batch(
    chunk_ids: &[String],
    vectors: &[Vec<f32>],
    dim: usize,
) -> Result<RecordBatch> {
    let id_refs: Vec<&str> = chunk_ids.iter().map(|s| s.as_str()).collect();
    let id_col = Arc::new(StringArray::from(id_refs));

    let total_elements = vectors.len() * dim;
    let mut flat_floats = Vec::with_capacity(total_elements);

    for vec in vectors {
        if vec.len() != dim {
            anyhow::bail!(
                "embedding dimension mismatch: expected {}, got {}",
                dim,
                vec.len()
            );
        }
        flat_floats.extend_from_slice(vec);
    }

    let float_array = Arc::new(Float32Array::from(flat_floats));
    let list_field = Arc::new(Field::new("item", DataType::Float32, true));
    let list_col = Arc::new(FixedSizeListArray::new(
        list_field,
        dim as i32,
        float_array,
        None,
    ));

    let schema = embeddings_schema(dim as i32);
    let cols: Vec<ArrayRef> = vec![id_col, list_col];

    RecordBatch::try_new(schema, cols).context("failed to construct embeddings RecordBatch")
}
