use std::sync::atomic::Ordering;

use crate::embedding_contract::CHUNK_MODEL_ID as CHUNK_EMBEDDING_MODEL_ID;
use crate::file_ingress_guard::FileIngressRow;
use crate::graph::{GraphStore, PendingFile};

use super::{FileUpsertSource, DEFAULT_GRAPH_EMBEDDING_RADIUS, FILE_VECTORIZATION_CLAIM_SEQ};

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

pub(super) fn parse_pending_file_row(row: Vec<serde_json::Value>) -> Option<PendingFile> {
    if row.len() < 6 {
        return None;
    }

    let priority = parse_i64_field(&row[2])?;
    let size_bytes = parse_u64_field(&row[3]).unwrap_or(0);
    let defer_count = parse_u64_field(&row[4]).unwrap_or(0).min(u32::MAX as u64) as u32;
    let last_deferred_at_ms = parse_i64_field(&row[5]);

    Some(PendingFile {
        path: row[0].as_str()?.to_string(),
        trace_id: row[1].as_str()?.to_string(),
        priority,
        size_bytes,
        defer_count,
        last_deferred_at_ms,
    })
}

pub(super) fn parse_file_ingress_row(row: Vec<serde_json::Value>) -> Option<FileIngressRow> {
    if row.len() < 7 {
        return None;
    }

    Some(FileIngressRow {
        path: row[0].as_str()?.to_string(),
        status: row[1].as_str()?.to_string(),
        mtime: parse_i64_field(&row[2]).unwrap_or_default(),
        size: parse_i64_field(&row[3]).unwrap_or_default(),
        file_stage: row[4].as_str().unwrap_or_default().to_string(),
        status_reason: row[5].as_str().unwrap_or_default().to_string(),
        graph_ready: row[6].as_bool().unwrap_or(false),
    })
}

pub(super) fn graph_projection_queue_upsert(
    anchor_type: &str,
    anchor_id: &str,
    radius: i64,
    now_ms: i64,
) -> String {
    let safe_anchor_type = anchor_type.replace('\'', "''");
    let safe_anchor_id = anchor_id.replace('\'', "''");
    format!(
        "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at, last_error_reason, last_attempt_at) \
         VALUES ('{}', '{}', {}, 'queued', 0, {}, NULL, NULL) \
         ON CONFLICT(anchor_type, anchor_id, radius) DO UPDATE \
         SET status = 'queued', \
             attempts = 0, \
             queued_at = {}, \
             last_error_reason = NULL, \
             last_attempt_at = NULL;",
        safe_anchor_type,
        safe_anchor_id,
        radius,
        now_ms,
        now_ms
    )
}

pub(super) fn graph_projection_queue_upsert_if_needed_for_file(
    file_path: &str,
    now_ms: i64,
) -> String {
    let safe_path = file_path.replace('\'', "''");
    format!(
        "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at, last_error_reason, last_attempt_at) \
         SELECT 'file', path, {DEFAULT_GRAPH_EMBEDDING_RADIUS}, 'queued', 0, {now_ms}, NULL, NULL \
         FROM File \
         WHERE path = '{safe_path}' \
           AND status = 'pending' \
           AND COALESCE(file_stage, 'promoted') = 'promoted' \
           AND COALESCE(graph_ready, FALSE) = FALSE \
           AND COALESCE(vector_ready, FALSE) = FALSE \
           AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
         ON CONFLICT(anchor_type, anchor_id, radius) DO UPDATE \
         SET status = 'queued', \
             attempts = 0, \
             queued_at = {now_ms}, \
             last_error_reason = NULL, \
             last_attempt_at = NULL;"
    )
}

pub(super) fn file_vectorization_queue_upsert_if_needed(file_path: &str, now_ms: i64) -> String {
    let safe_path = file_path.replace('\'', "''");
    format!(
        "INSERT INTO FileVectorizationQueue (file_path, status, status_reason, attempts, queued_at, last_error_reason, last_attempt_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
         SELECT path, 'queued', NULL, 0, {}, NULL, NULL, NULL, NULL, NULL, NULL, 0 \
         FROM File \
         WHERE path = '{}' \
           AND graph_ready = TRUE \
           AND vector_ready = FALSE \
           AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
           AND file_stage NOT IN ('deleted', 'skipped', 'oversized') \
         ON CONFLICT(file_path) DO UPDATE \
         SET status = 'queued', \
         status_reason = NULL, \
         attempts = 0, \
         queued_at = {}, \
         last_error_reason = NULL, \
         last_attempt_at = NULL, \
         claim_token = NULL, \
         claimed_at_ms = NULL, \
         lease_heartbeat_at_ms = NULL, \
         lease_owner = NULL, \
         lease_epoch = 0",
        now_ms, safe_path, now_ms
    )
}

pub(super) fn orphaned_file_vectorization_candidates_query(
    limit: Option<usize>,
    paths: Option<&[String]>,
) -> String {
    let path_filter = paths
        .filter(|paths| !paths.is_empty())
        .map(|paths| {
            let escaped = paths
                .iter()
                .map(|path| format!("'{}'", GraphStore::escape_sql(path)))
                .collect::<Vec<_>>()
                .join(",");
            format!(" AND path IN ({escaped})")
        })
        .unwrap_or_default();
    let limit_clause = limit
        .filter(|value| *value > 0)
        .map(|value| format!(" LIMIT {value}"))
        .unwrap_or_default();

    format!(
        "SELECT path \
         FROM File \
         WHERE status IN ('indexed', 'indexed_degraded') \
           AND file_stage = 'graph_indexed' \
           AND graph_ready = TRUE \
           AND vector_ready = FALSE \
           AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
           AND file_stage NOT IN ('deleted', 'skipped', 'oversized') \
           AND NOT EXISTS ( \
               SELECT 1 \
               FROM FileVectorizationQueue fvq \
               WHERE fvq.file_path = File.path \
           ) \
           AND EXISTS ( \
               SELECT 1 \
               FROM Chunk c \
               LEFT JOIN ChunkEmbedding ce \
                 ON ce.chunk_id = c.id \
                AND ce.model_id = '{model_id}' \
                AND ce.source_hash = c.content_hash \
               WHERE c.file_path = File.path \
                 AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
           ){path_filter} \
         ORDER BY COALESCE(priority, 0) DESC, \
                  COALESCE(graph_ready_at_ms, last_state_change_at_ms, mtime, 0) ASC, \
                  path ASC{limit_clause}",
        model_id = GraphStore::escape_sql(CHUNK_EMBEDDING_MODEL_ID),
    )
}

pub(super) fn orphaned_file_vectorization_requeue_sql(
    now_ms: i64,
    limit: Option<usize>,
    paths: Option<&[String]>,
) -> String {
    let candidates = orphaned_file_vectorization_candidates_query(limit, paths);
    format!(
        "INSERT INTO FileVectorizationQueue (file_path, status, status_reason, attempts, queued_at, last_error_reason, last_attempt_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
         SELECT candidate.path, 'queued', 'reconciled_orphan_vectorization_state', 0, {now_ms}, NULL, NULL, NULL, NULL, NULL, NULL, 0 \
         FROM ({candidates}) candidate \
         ON CONFLICT(file_path) DO UPDATE \
         SET status = 'queued', \
             status_reason = 'reconciled_orphan_vectorization_state', \
             attempts = 0, \
             queued_at = {now_ms}, \
             last_error_reason = NULL, \
             last_attempt_at = NULL, \
             claim_token = NULL, \
             claimed_at_ms = NULL, \
             lease_heartbeat_at_ms = NULL, \
             lease_owner = NULL, \
             lease_epoch = 0"
    )
}

pub(super) fn hourly_bucket_start_ms(at_ms: i64) -> i64 {
    (at_ms / 3_600_000) * 3_600_000
}

pub(super) fn next_vector_persist_outbox_claim_token(now_ms: i64) -> String {
    let seq = FILE_VECTORIZATION_CLAIM_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("outbox-claim-{}-{}", now_ms, seq)
}

pub(super) fn sort_and_dedup_sql_tuples(values: &mut Vec<String>) {
    values.sort_unstable();
    values.dedup();
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

pub(super) fn dedup_file_batch_rows(
    rows: &[(String, String, i64, i64, i64, FileUpsertSource)],
) -> Vec<(String, String, i64, i64, i64, FileUpsertSource)> {
    let mut deduped = std::collections::BTreeMap::new();
    for (path, project, size, mtime, priority, source) in rows {
        deduped.insert(
            path.clone(),
            (project.clone(), *size, *mtime, *priority, *source),
        );
    }

    deduped
        .into_iter()
        .map(|(path, (project, size, mtime, priority, source))| {
            (path, project, size, mtime, priority, source)
        })
        .collect()
}
