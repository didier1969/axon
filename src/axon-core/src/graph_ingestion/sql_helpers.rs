// REQ-AXO-901653 slice-5c — sql_helpers trimmed to live methods only.
// Deleted helpers (zero callers post worker.rs / file_ingress / queue purge) :
// parse_pending_file_row, parse_file_ingress_row, graph_projection_queue_upsert,
// graph_projection_queue_upsert_if_needed_for_file,
// file_vectorization_queue_upsert_if_needed,
// orphaned_file_vectorization_candidates_query,
// orphaned_file_vectorization_requeue_sql, hourly_bucket_start_ms,
// next_vector_persist_outbox_claim_token, dedup_file_batch_rows.

/// REQ-AXO-238 / REQ-AXO-193 E.7: shared SQL string escape used by the
/// async_writer typed-row renderers. Mirrors `GraphStore::escape_sql` so
/// the rendered INSERT statements are bit-for-bit identical to the
/// producer's legacy output.
pub(super) fn escape_sql_text(value: &str) -> String {
    value.replace('\0', " ").replace('\'', "''")
}

pub(super) fn parse_i64_field(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|v| v.min(i64::MAX as u64) as i64))
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

pub(super) fn parse_u64_field(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().map(|v| v.max(0) as u64))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

pub(super) fn insert_unique_relation_queries(table: &str, values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|row| {
            format!(
                "INSERT INTO {table} (source_id, target_id, project_code) VALUES {row} \
                 ON CONFLICT DO NOTHING;",
                table = table,
                row = row
            )
        })
        .collect()
}

pub(super) fn replace_relation_queries(
    table: &str,
    values: &[String],
    chunk_size: usize,
) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for chunk in values.chunks(chunk_size.max(1)) {
        let values_sql = chunk.join(",");
        queries.push(format!(
            "DELETE FROM {table} USING (VALUES {values_sql}) AS incoming(source_id, target_id, project_code) \
             WHERE {table}.source_id = incoming.source_id \
               AND {table}.target_id = incoming.target_id \
               AND {table}.project_code = incoming.project_code;",
            table = table,
            values_sql = values_sql
        ));
        queries.push(format!(
            "INSERT INTO {table} (source_id, target_id, project_code) VALUES {values_sql};",
            table = table,
            values_sql = values_sql
        ));
    }

    queries
}
