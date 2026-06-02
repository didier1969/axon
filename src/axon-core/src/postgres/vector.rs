//! MIL-AXO-015 P4 — pgvector helpers for the ChunkEmbedding lane.
//!
//! Centralises the SQL-text construction needed to write and read
//! `vector(1024)` rows on PostgreSQL via the existing
//! `pg_execute / pg_query_json` FFI surface (no parameter binding for
//! vector types because tokio-postgres' `&[f32]` adapter is gated
//! behind the pgvector crate's runtime types lookup, which is overkill
//! for our inline-literal write pattern).
//!
//! Format: pgvector accepts `'[v1, v2, …, vN]'::vector(N)` text cast.
//! Float values are rendered with `{:.7}` precision (matches the
//! pgvector::Vector default) and the array is delimited by literal
//! square brackets.
//!
//! Wire-up status (2026-05-07): callers in `vector_worker_loop` and
//! `retrieve_context` still emit DuckDB-shaped writes. The actual
//! switch is the next P4 slice; this module provides the building
//! blocks so the diff stays small and reviewable.

use crate::embedding_contract::DIMENSION;

/// Render a `Vec<f32>` as a pgvector text literal, including the
/// `::vector(N)` cast. Returns `'[…]'::vector(1024)`.
///
/// Single-quote escaping is unnecessary because the literal contains
/// only digits, dots, commas, brackets, and ASCII minus signs — none
/// of which terminate a SQL string. The function still validates that
/// the embedding has exactly `DIMENSION` components so a caller cannot
/// accidentally write a mismatched-rank vector.
pub fn vector_literal(values: &[f32]) -> Result<String, VectorError> {
    if values.len() != DIMENSION {
        return Err(VectorError::WrongDimension {
            expected: DIMENSION,
            actual: values.len(),
        });
    }
    let mut buf = String::with_capacity(values.len() * 10 + 32);
    buf.push('\'');
    buf.push('[');
    for (idx, v) in values.iter().enumerate() {
        if idx > 0 {
            buf.push(',');
        }
        // Use the fixed-point form; pgvector parses the standard f32
        // text rendering. Avoid scientific notation — pgvector accepts
        // it but the output stays human-greppable.
        if v.is_finite() {
            // {:.7} keeps enough precision for f32 round-trip while
            // staying compact.
            use std::fmt::Write;
            let _ = write!(buf, "{:.7}", v);
        } else {
            return Err(VectorError::NonFiniteValue {
                index: idx,
                value: *v,
            });
        }
    }
    buf.push(']');
    buf.push('\'');
    buf.push_str(&format!("::vector({})", DIMENSION));
    Ok(buf)
}

/// Inverse of `vector_literal` for tests + the few read paths that
/// currently surface vector text via `pg_query_json` (which renders
/// the agtype-like row as a string).
pub fn parse_vector_text(text: &str) -> Result<Vec<f32>, VectorError> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or(VectorError::MalformedLiteral)?;
    let mut out = Vec::with_capacity(DIMENSION);
    for piece in inner.split(',') {
        let raw = piece.trim();
        if raw.is_empty() {
            continue;
        }
        let v: f32 = raw.parse().map_err(|_| VectorError::MalformedLiteral)?;
        out.push(v);
    }
    if out.len() != DIMENSION {
        return Err(VectorError::WrongDimension {
            expected: DIMENSION,
            actual: out.len(),
        });
    }
    Ok(out)
}

/// Build the upsert statement that vector_worker_loop will issue
/// per-chunk under the PostgreSQL backend. The rest of the row is
/// inlined as quoted strings since pg_execute does not bind parameters.
///
/// Post-CPT-AXO-039 supersedure (2026-05-08): targets `ist.ChunkEmbedding`
/// with a `project_code` row column scoping the entry, instead of a
/// per-project schema. Multi-project queries become a simple
/// `WHERE project_code IN (...)` filter rather than UNION ALL.
///
/// `chunk_id`, `model_id`, `project_code`, `source_hash` are
/// caller-provided strings — the function escapes single quotes before
/// composing the SQL. None of the IDs in Axon contain SQL terminators
/// today, but the escape keeps the helper safe against future format
/// drift.
pub fn upsert_chunk_embedding_sql(
    chunk_id: &str,
    model_id: &str,
    project_code: &str,
    source_hash: &str,
    embedding: &[f32],
    embedded_at_ms: i64,
) -> Result<String, VectorError> {
    let vec_lit = vector_literal(embedding)?;
    Ok(format!(
        "INSERT INTO ist.ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
         VALUES ('{}', '{}', '{}', '{}', {}, {}) \
         ON CONFLICT (chunk_id, model_id) DO UPDATE SET \
         project_code = EXCLUDED.project_code, \
         source_hash = EXCLUDED.source_hash, \
         embedding = EXCLUDED.embedding, \
         embedded_at_ms = EXCLUDED.embedded_at_ms",
        sql_escape(chunk_id),
        sql_escape(model_id),
        sql_escape(project_code),
        sql_escape(source_hash),
        vec_lit,
        embedded_at_ms,
    ))
}

/// Build a HNSW-backed cosine similarity ANN query body for the
/// post-CPT-AXO-039 multi-project layout. Returns the
/// `FROM ist.ChunkEmbedding WHERE ... ORDER BY ... LIMIT ...`
/// segment scoped to a single project.
///
/// Caller composes the SELECT projection — typically `chunk_id` plus
/// optional joins — and prepends it to the segment returned here.
pub fn cosine_ann_where_order_limit(
    model_id: &str,
    project_code: &str,
    query: &[f32],
    limit: usize,
) -> Result<String, VectorError> {
    let vec_lit = vector_literal(query)?;
    Ok(format!(
        "FROM ist.ChunkEmbedding \
         WHERE model_id = '{}' AND project_code = '{}' \
         ORDER BY embedding <=> {} \
         LIMIT {}",
        sql_escape(model_id),
        sql_escape(project_code),
        vec_lit,
        limit,
    ))
}

#[derive(Debug)]
pub enum VectorError {
    WrongDimension { expected: usize, actual: usize },
    NonFiniteValue { index: usize, value: f32 },
    MalformedLiteral,
}

impl std::fmt::Display for VectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VectorError::WrongDimension { expected, actual } => write!(
                f,
                "vector dimension mismatch: expected {expected}, got {actual}"
            ),
            VectorError::NonFiniteValue { index, value } => write!(
                f,
                "vector contained non-finite value at index {index}: {value}"
            ),
            VectorError::MalformedLiteral => write!(f, "malformed pgvector text literal"),
        }
    }
}

impl std::error::Error for VectorError {}

fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vector(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0_f32; DIMENSION];
        v[0] = seed;
        v[1] = seed + 0.1;
        v[DIMENSION - 1] = -seed;
        v
    }

    #[test]
    fn literal_round_trip_preserves_values() {
        let v = sample_vector(0.5);
        let lit = vector_literal(&v).unwrap();
        assert!(lit.starts_with("'["));
        assert!(lit.contains(&format!("::vector({DIMENSION})")));
        // Extract the bracketed array body from `'[…]'::vector(1024)`:
        // skip the opening apostrophe, then take everything up to the
        // closing apostrophe.
        let inner_with_brackets = lit
            .trim_start_matches('\'')
            .split('\'')
            .next()
            .expect("lit has at least the array body");
        let parsed = parse_vector_text(inner_with_brackets).unwrap();
        assert_eq!(parsed.len(), DIMENSION);
        assert!((parsed[0] - 0.5).abs() < 1e-6);
        assert!((parsed[1] - 0.6).abs() < 1e-6);
        assert!((parsed[DIMENSION - 1] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn literal_rejects_wrong_dimension() {
        let short = vec![1.0_f32; DIMENSION - 1];
        assert!(matches!(
            vector_literal(&short),
            Err(VectorError::WrongDimension { .. })
        ));
        let long = vec![1.0_f32; DIMENSION + 1];
        assert!(matches!(
            vector_literal(&long),
            Err(VectorError::WrongDimension { .. })
        ));
    }

    #[test]
    fn literal_rejects_non_finite() {
        let mut v = sample_vector(0.0);
        v[42] = f32::NAN;
        assert!(matches!(
            vector_literal(&v),
            Err(VectorError::NonFiniteValue { index: 42, .. })
        ));
        v[42] = f32::INFINITY;
        assert!(matches!(
            vector_literal(&v),
            Err(VectorError::NonFiniteValue { index: 42, .. })
        ));
    }

    #[test]
    fn parse_text_handles_whitespace_and_signs() {
        let mut input = String::from("[ ");
        for i in 0..DIMENSION {
            if i > 0 {
                input.push(',');
            }
            if i == 0 {
                input.push_str("-1.0");
            } else {
                input.push_str("0.0");
            }
        }
        input.push(']');
        let parsed = parse_vector_text(&input).unwrap();
        assert_eq!(parsed.len(), DIMENSION);
        assert!((parsed[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn upsert_sql_includes_on_conflict_and_project_code() {
        let v = sample_vector(0.25);
        let sql = upsert_chunk_embedding_sql(
            "chunk-x",
            "code-1024",
            "AXO",
            "hash-abc",
            &v,
            1714999999000,
        )
        .unwrap();
        assert!(sql.contains("INSERT INTO ist.ChunkEmbedding"));
        assert!(sql.contains("ON CONFLICT (chunk_id, model_id) DO UPDATE"));
        assert!(sql.contains("'chunk-x'"));
        assert!(sql.contains("'code-1024'"));
        assert!(sql.contains("'AXO'"));
        assert!(sql.contains("'hash-abc'"));
        assert!(sql.contains("1714999999000"));
        assert!(sql.contains(&format!("::vector({DIMENSION})")));
        assert!(sql.contains("project_code = EXCLUDED.project_code"));
    }

    #[test]
    fn upsert_sql_escapes_single_quotes_in_ids() {
        let v = sample_vector(0.0);
        let sql =
            upsert_chunk_embedding_sql("id'with'quotes", "m", "AXO", "h", &v, 0).unwrap();
        // The escaped sequence appears (single quotes doubled).
        assert!(sql.contains("'id''with''quotes'"));
    }

    #[test]
    fn cosine_query_uses_pgvector_distance_operator_and_project_filter() {
        let v = sample_vector(1.0);
        let body = cosine_ann_where_order_limit("code-1024", "AXO", &v, 10).unwrap();
        assert!(body.contains("FROM ist.ChunkEmbedding"));
        assert!(body.contains("WHERE model_id = 'code-1024' AND project_code = 'AXO'"));
        assert!(body.contains("<=>"));
        assert!(body.ends_with("LIMIT 10"));
    }
}
