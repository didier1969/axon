use anyhow::Result;

use crate::graph::GraphStore;

use super::{graph_projection_queue_upsert, GraphProjectionWork, DEFAULT_GRAPH_EMBEDDING_RADIUS};

impl GraphStore {
    pub fn enqueue_graph_projection_refresh(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
    ) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&graph_projection_queue_upsert(
            anchor_type,
            anchor_id,
            radius,
            now_ms,
        ))
    }

    pub fn enqueue_graph_projection_refresh_batch(&self, work: &[(&str, &str, i64)]) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let queries = work
            .iter()
            .map(|(anchor_type, anchor_id, radius)| {
                graph_projection_queue_upsert(anchor_type, anchor_id, *radius, now_ms)
            })
            .collect::<Vec<_>>();

        self.execute_batch(&queries)
    }

    pub fn fetch_pending_graph_projection_work(
        &self,
        count: usize,
    ) -> Result<Vec<GraphProjectionWork>> {
        if count == 0 {
            return Ok(Vec::new());
        }

        let query = format!(
            "SELECT anchor_type, anchor_id, radius \
             FROM GraphProjectionQueue \
             WHERE status = 'queued' \
             ORDER BY COALESCE(queued_at, 0), anchor_type, anchor_id \
             LIMIT {}",
            count
        );
        let raw = self.query_json(&query)?;

        if raw == "[]" || raw.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut queue = Vec::new();
        for row in rows {
            let Some(anchor_type) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(anchor_id) = row.get(1).and_then(|value| value.as_str()) else {
                continue;
            };
            let radius = row
                .get(2)
                .and_then(|value| value.as_i64())
                .unwrap_or(DEFAULT_GRAPH_EMBEDDING_RADIUS);

            queue.push(GraphProjectionWork {
                anchor_type: anchor_type.to_string(),
                anchor_id: anchor_id.to_string(),
                radius,
            });
        }

        if queue.is_empty() {
            return Ok(queue);
        }

        let predicates = queue
            .iter()
            .map(|item| {
                format!(
                    "(anchor_type = '{}' AND anchor_id = '{}' AND radius = {})",
                    Self::escape_sql(&item.anchor_type),
                    Self::escape_sql(&item.anchor_id),
                    item.radius
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE GraphProjectionQueue \
             SET status = 'inflight', \
                 last_attempt_at = {}, \
                 attempts = attempts + 1 \
             WHERE status = 'queued' AND ({})",
            chrono::Utc::now().timestamp_millis(),
            predicates
        ))?;
        Ok(queue)
    }

    pub fn mark_graph_projection_work_done(&self, work: &[GraphProjectionWork]) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| {
                format!(
                    "(anchor_type = '{}' AND anchor_id = '{}' AND radius = {})",
                    Self::escape_sql(&item.anchor_type),
                    Self::escape_sql(&item.anchor_id),
                    item.radius
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "DELETE FROM GraphProjectionQueue \
             WHERE status = 'inflight' AND ({})",
            predicates
        ))
    }

    pub fn mark_graph_projection_work_failed(
        &self,
        work: &[GraphProjectionWork],
        reason: &str,
    ) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| {
                format!(
                    "(anchor_type = '{}' AND anchor_id = '{}' AND radius = {})",
                    Self::escape_sql(&item.anchor_type),
                    Self::escape_sql(&item.anchor_id),
                    item.radius
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE GraphProjectionQueue \
             SET status = 'queued', \
                 last_error_reason = '{}', \
                 last_attempt_at = {}, \
                 attempts = attempts + 1 \
             WHERE status = 'inflight' AND ({})",
            Self::escape_sql(reason),
            chrono::Utc::now().timestamp_millis(),
            predicates
        ))
    }

    pub fn clear_stale_inflight_graph_projection_work(&self) -> Result<()> {
        self.execute(
            "UPDATE GraphProjectionQueue \
             SET status = 'queued' \
             WHERE status = 'inflight'",
        )
    }

    pub fn backfill_graph_projection_queue_for_model(&self, model_id: &str) -> Result<usize> {
        let query = format!(
            "SELECT gps.anchor_type, gps.anchor_id, gps.radius \
             FROM GraphProjectionState gps \
             LEFT JOIN GraphEmbedding ge \
               ON ge.anchor_type = gps.anchor_type \
              AND ge.anchor_id = gps.anchor_id \
              AND ge.radius = gps.radius \
              AND ge.model_id = '{}' \
             WHERE ge.anchor_id IS NULL \
                OR ge.source_signature <> gps.source_signature \
                OR ge.projection_version <> gps.projection_version",
            Self::escape_sql(model_id)
        );
        let raw = self.query_json(&query)?;
        if raw == "[]" || raw.is_empty() {
            return Ok(0);
        }

        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            return Ok(0);
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut queries = Vec::new();
        for row in rows {
            let Some(anchor_type) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(anchor_id) = row.get(1).and_then(|value| value.as_str()) else {
                continue;
            };
            let radius = row
                .get(2)
                .and_then(|value| value.as_i64())
                .unwrap_or(DEFAULT_GRAPH_EMBEDDING_RADIUS);
            queries.push(graph_projection_queue_upsert(
                anchor_type,
                anchor_id,
                radius,
                now_ms,
            ));
        }

        let inserted = queries.len();
        if inserted == 0 {
            return Ok(0);
        }

        self.execute_batch(&queries)?;
        Ok(inserted)
    }

    pub fn fetch_graph_projection_queue_counts(&self) -> Result<(usize, usize)> {
        let queued =
            self.query_count("SELECT count(*) FROM GraphProjectionQueue WHERE status = 'queued'")?;
        let inflight = self
            .query_count("SELECT count(*) FROM GraphProjectionQueue WHERE status = 'inflight'")?;
        let queued = usize::try_from(queued).unwrap_or(0);
        let inflight = usize::try_from(inflight).unwrap_or(0);
        Ok((queued, inflight))
    }
}
