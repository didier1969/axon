use anyhow::{anyhow, Result};

use crate::graph::GraphStore;
use crate::runtime_mode::AxonRuntimeMode;
use crate::service_guard;

use super::{
    file_vectorization_queue_upsert_if_needed, orphaned_file_vectorization_candidates_query,
    orphaned_file_vectorization_requeue_sql, parse_u64_field, FileVectorizationLeaseSnapshot,
    FileVectorizationWork,
};

impl GraphStore {
    pub fn enqueue_file_vectorization_refresh(&self, file_path: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&file_vectorization_queue_upsert_if_needed(
            file_path, now_ms,
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(())
    }

    pub fn fetch_pending_file_vectorization_work(
        &self,
        count: usize,
    ) -> Result<Vec<FileVectorizationWork>> {
        if count == 0 {
            return Ok(Vec::new());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let claim_token = Self::next_file_vectorization_claim_token(now_ms);
        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'inflight', \
                 status_reason = CASE \
                     WHEN status = 'paused_for_interactive_priority' THEN 'resumed_after_interactive_pause' \
                     ELSE NULL \
                 END, \
                 next_eligible_at_ms = NULL, \
                 last_attempt_at = {}, \
                 attempts = attempts + 1, \
                 claim_token = '{}', \
                 claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {}, \
                 lease_owner = 'vector', \
                 lease_epoch = COALESCE(lease_epoch, 0) \
             WHERE status IN ('queued', 'paused_for_interactive_priority') \
               AND file_path IN ( \
                   {} \
                   ORDER BY COALESCE(queued_at, 0), fq.file_path \
                   LIMIT {} \
               )",
            now_ms,
            Self::escape_sql(&claim_token),
            now_ms,
            now_ms,
            Self::claimable_file_vectorization_candidates_query(now_ms),
            count
        ))?;

        let raw = self.query_json_writer(&format!(
            "SELECT file_path, COALESCE(status_reason, '') \
             FROM FileVectorizationQueue \
             WHERE claim_token = '{}' \
             ORDER BY COALESCE(queued_at, 0), file_path",
            Self::escape_sql(&claim_token)
        ))?;

        if raw == "[]" || raw.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut queue = Vec::new();
        for row in rows {
            let Some(file_path) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };

            let resumed_after_interactive_pause = row
                .get(1)
                .and_then(|value| value.as_str())
                .map(|value| value == "resumed_after_interactive_pause")
                .unwrap_or(false);

            queue.push(FileVectorizationWork {
                file_path: file_path.to_string(),
                resumed_after_interactive_pause,
            });
        }

        Ok(queue)
    }

    pub fn mark_file_vectorization_started(&self, work: &[FileVectorizationWork]) -> Result<usize> {
        if work.is_empty() {
            return Ok(0);
        }

        let predicates = work
            .iter()
            .map(|item| format!("(path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let started = usize::try_from(self.query_count(&format!(
            "SELECT count(*) \
             FROM File \
             WHERE vectorization_started_at_ms IS NULL \
               AND ({})",
            predicates
        ))?)
        .unwrap_or(0);

        self.execute(&format!(
            "UPDATE File \
             SET vectorization_started_at_ms = COALESCE(vectorization_started_at_ms, {}), \
                 last_state_change_at_ms = {} \
             WHERE ({})",
            now_ms, now_ms, predicates
        ))?;

        Ok(started)
    }

    pub fn pause_file_vectorization_work_for_interactive_priority(
        &self,
        work: &[FileVectorizationWork],
        cooldown_ms: i64,
        max_interruptions: i64,
    ) -> Result<usize> {
        if work.is_empty() {
            return Ok(0);
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let next_eligible_at_ms = now_ms.saturating_add(cooldown_ms.max(0));
        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        let affected_query = format!(
            "SELECT count(*) \
             FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND COALESCE(interactive_pause_count, 0) < {} \
               AND ({})",
            max_interruptions.max(0),
            predicates
        );
        let affected = usize::try_from(self.query_count_writer(&affected_query)?).unwrap_or(0);

        if affected == 0 {
            return Ok(0);
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'paused_for_interactive_priority', \
                 status_reason = 'requeued_for_interactive_priority', \
                 last_error_reason = 'requeued_for_interactive_priority', \
                 next_eligible_at_ms = {}, \
                 interactive_pause_count = COALESCE(interactive_pause_count, 0) + 1, \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight' \
               AND COALESCE(interactive_pause_count, 0) < {} \
               AND ({})",
            next_eligible_at_ms,
            max_interruptions.max(0),
            predicates
        ))?;

        Ok(affected)
    }

    pub fn mark_file_vectorization_persist_started(
        &self,
        works: &[FileVectorizationWork],
    ) -> Result<()> {
        if works.is_empty() {
            return Ok(());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let predicates = works
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET persist_started_at_ms = {} \
             WHERE status = 'inflight' \
               AND lease_owner = 'vector' \
               AND ({})",
            now_ms, predicates
        ))
    }

    pub fn mark_file_vectorization_work_done(&self, work: &[FileVectorizationWork]) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "DELETE FROM FileVectorizationQueue \
             WHERE ({})",
            predicates
        ))
    }

    pub fn refresh_inflight_file_vectorization_claims(
        &self,
        work: &[FileVectorizationWork],
    ) -> Result<usize> {
        self.refresh_file_vectorization_leases_for_owner(work, "vector")
    }

    pub fn refresh_file_vectorization_leases_for_owner(
        &self,
        work: &[FileVectorizationWork],
        lease_owner: &str,
    ) -> Result<usize> {
        if work.is_empty() {
            return Ok(0);
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let refreshed = usize::try_from(self.query_count_writer(&format!(
            "SELECT count(*) FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            Self::escape_sql(lease_owner),
            predicates
        ))?)
        .unwrap_or(0);

        if refreshed == 0 {
            return Ok(0);
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {}, \
                 last_attempt_at = {} \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            now_ms,
            now_ms,
            now_ms,
            Self::escape_sql(lease_owner),
            predicates
        ))?;

        Ok(refreshed)
    }

    pub fn transfer_file_vectorization_lease_owner(
        &self,
        snapshots: &[FileVectorizationLeaseSnapshot],
        from_owner: &str,
        to_owner: &str,
    ) -> Result<Vec<FileVectorizationLeaseSnapshot>> {
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }

        let predicates = snapshots
            .iter()
            .map(|item| {
                format!(
                    "(file_path = '{}' AND claim_token = '{}' AND COALESCE(lease_epoch, 0) = {})",
                    Self::escape_sql(&item.file_path),
                    Self::escape_sql(&item.claim_token),
                    item.lease_epoch
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let transferred = usize::try_from(self.query_count_writer(&format!(
            "SELECT count(*) FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            Self::escape_sql(from_owner),
            predicates
        ))?)
        .unwrap_or(0);

        if transferred != snapshots.len() {
            return Err(anyhow!(
                "lease owner transfer refused: expected {} rows, matched {}",
                snapshots.len(),
                transferred
            ));
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET lease_owner = '{}', \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1, \
                 lease_heartbeat_at_ms = {}, \
                 last_attempt_at = {} \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            Self::escape_sql(to_owner),
            now_ms,
            now_ms,
            Self::escape_sql(from_owner),
            predicates
        ))?;

        Ok(snapshots
            .iter()
            .map(|item| FileVectorizationLeaseSnapshot {
                file_path: item.file_path.clone(),
                claim_token: item.claim_token.clone(),
                lease_epoch: item.lease_epoch.saturating_add(1),
            })
            .collect())
    }

    pub fn capture_file_vectorization_lease_snapshots(
        &self,
        work: &[FileVectorizationWork],
        lease_owner: &str,
    ) -> Result<Vec<FileVectorizationLeaseSnapshot>> {
        if work.is_empty() {
            return Ok(Vec::new());
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let raw = self.query_json_writer(&format!(
            "SELECT file_path, claim_token, COALESCE(lease_epoch, 0) \
             FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({}) \
             ORDER BY file_path",
            Self::escape_sql(lease_owner),
            predicates
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut snapshots = rows
            .into_iter()
            .filter_map(|row| {
                let file_path = row.first()?.as_str()?.to_string();
                let claim_token = row.get(1)?.as_str()?.to_string();
                let lease_epoch = parse_u64_field(row.get(2)?).unwrap_or(0);
                Some(FileVectorizationLeaseSnapshot {
                    file_path,
                    claim_token,
                    lease_epoch,
                })
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|a, b| a.file_path.cmp(&b.file_path));

        let mut expected_paths = work
            .iter()
            .map(|item| item.file_path.clone())
            .collect::<Vec<_>>();
        expected_paths.sort();
        let actual_paths = snapshots
            .iter()
            .map(|item| item.file_path.clone())
            .collect::<Vec<_>>();
        if actual_paths != expected_paths {
            return Err(anyhow!(
                "lease snapshot capture mismatch: expected {:?}, got {:?}",
                expected_paths,
                actual_paths
            ));
        }

        Ok(snapshots)
    }

    pub fn mark_file_vectorization_work_failed(
        &self,
        work: &[FileVectorizationWork],
        reason: &str,
    ) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'queued', \
                status_reason = NULL, \
                last_error_reason = '{}', \
                last_attempt_at = {}, \
                attempts = attempts + 1, \
                claim_token = NULL, \
                claimed_at_ms = NULL, \
                lease_heartbeat_at_ms = NULL, \
                lease_owner = NULL, \
                lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight' AND ({})",
            Self::escape_sql(reason),
            chrono::Utc::now().timestamp_millis(),
            predicates
        ))?;
        self.execute(&format!(
            "UPDATE File \
             SET last_error_reason = '{}', \
                 last_error_at_ms = {}, \
                 last_state_change_at_ms = {} \
             WHERE ({})",
            Self::escape_sql(reason),
            chrono::Utc::now().timestamp_millis(),
            chrono::Utc::now().timestamp_millis(),
            predicates.replace("file_path", "path")
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(())
    }

    pub fn clear_stale_inflight_file_vectorization_work(&self) -> Result<()> {
        let recovered = self.query_count_writer(
            "SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'",
        )?;
        self.execute(
            "UPDATE FileVectorizationQueue \
             SET status = 'queued', \
                 status_reason = 'recovered_after_stale_inflight', \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight'",
        )?;
        if recovered > 0 {
            service_guard::notify_vector_backlog_activity();
        }
        Ok(())
    }

    pub fn recover_stale_inflight_file_vectorization_work(
        &self,
        now_ms: i64,
        max_claim_age_ms: i64,
    ) -> Result<usize> {
        let cutoff_ms = now_ms.saturating_sub(max_claim_age_ms.max(0));
        let recovered = usize::try_from(self.query_count_writer(&format!(
            "SELECT count(*) \
             FROM FileVectorizationQueue fq \
             LEFT JOIN File f ON f.path = fq.file_path \
             WHERE fq.status = 'inflight' \
               AND COALESCE(f.vector_ready, FALSE) = FALSE \
               AND fq.claim_token IS NOT NULL \
               AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
               AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) IS NOT NULL \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) <= {} \
               AND (fq.persist_started_at_ms IS NULL OR fq.persist_started_at_ms <= {})",
            cutoff_ms, cutoff_ms
        ))?)
        .unwrap_or(0);

        if recovered == 0 {
            return Ok(0);
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'queued', \
                 status_reason = 'recovered_after_stale_inflight', \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight' \
               AND file_path IN ( \
                   SELECT fq.file_path \
                   FROM FileVectorizationQueue fq \
                   LEFT JOIN File f ON f.path = fq.file_path \
                   WHERE fq.status = 'inflight' \
                     AND COALESCE(f.vector_ready, FALSE) = FALSE \
                     AND fq.claim_token IS NOT NULL \
                     AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                     AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
                     AND COALESCE(fq.lease_heartbeat_at_ms, fq.claimed_at_ms) IS NOT NULL \
                     AND COALESCE(fq.lease_heartbeat_at_ms, fq.claimed_at_ms) <= {} \
                     AND (fq.persist_started_at_ms IS NULL OR fq.persist_started_at_ms <= {}) \
               ) \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) IS NOT NULL \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) <= {} \
               AND (persist_started_at_ms IS NULL OR persist_started_at_ms <= {})",
            cutoff_ms, cutoff_ms, cutoff_ms, cutoff_ms
        ))?;
        service_guard::notify_vector_backlog_activity();

        Ok(recovered)
    }

    pub fn backfill_file_vectorization_queue(&self) -> Result<usize> {
        self.reconcile_orphaned_file_vectorization_state(usize::MAX)
    }

    pub fn backfill_file_vectorization_queue_with_limit(&self, limit: usize) -> Result<usize> {
        if limit == 0 {
            return Ok(0);
        }
        self.reconcile_orphaned_file_vectorization_state(limit)
    }

    pub fn rebuild_file_vectorization_queue_with_limit(&self, limit: usize) -> Result<usize> {
        if limit == 0 {
            return Ok(0);
        }
        self.execute(
            "DELETE FROM FileVectorizationQueue \
             WHERE status IN ('queued', 'paused_for_interactive_priority')",
        )?;
        self.reconcile_orphaned_file_vectorization_state(limit)
    }

    pub fn count_orphaned_file_vectorization_files(&self) -> Result<usize> {
        let query = format!(
            "SELECT count(*) FROM ({}) orphaned",
            orphaned_file_vectorization_candidates_query(None, None)
        );
        Ok(usize::try_from(self.query_count_writer(&query)?).unwrap_or(0))
    }

    pub fn count_stale_inflight_file_vectorization_files(
        &self,
        now_ms: i64,
        stale_age_ms: i64,
    ) -> Result<usize> {
        let cutoff_ms = now_ms.saturating_sub(stale_age_ms.max(0));
        let query = format!(
            "SELECT count(*) \
             FROM FileVectorizationQueue fq \
             LEFT JOIN File f ON f.path = fq.file_path \
             WHERE fq.status = 'inflight' \
               AND fq.claim_token IS NOT NULL \
               AND COALESCE(f.vector_ready, FALSE) = FALSE \
               AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
               AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
               AND COALESCE(fq.lease_heartbeat_at_ms, fq.claimed_at_ms, 0) <= {cutoff_ms}"
        );
        Ok(usize::try_from(self.query_count_writer(&query)?).unwrap_or(0))
    }

    pub fn reconcile_orphaned_file_vectorization_state(&self, limit: usize) -> Result<usize> {
        self.reconcile_orphaned_file_vectorization_paths_internal(
            if limit == usize::MAX {
                None
            } else {
                Some(limit)
            },
            None,
        )
    }

    pub fn reconcile_orphaned_file_vectorization_paths(&self, paths: &[String]) -> Result<usize> {
        self.reconcile_orphaned_file_vectorization_paths_internal(None, Some(paths))
    }

    fn reconcile_orphaned_file_vectorization_paths_internal(
        &self,
        limit: Option<usize>,
        paths: Option<&[String]>,
    ) -> Result<usize> {
        if matches!(limit, Some(0)) {
            return Ok(0);
        }
        if paths.is_some_and(|paths| paths.is_empty()) {
            return Ok(0);
        }

        let candidate_count_query = format!(
            "SELECT count(*) FROM ({}) orphaned",
            orphaned_file_vectorization_candidates_query(limit, paths)
        );
        let candidate_count =
            usize::try_from(self.query_count_writer(&candidate_count_query)?).unwrap_or(0);
        if candidate_count == 0 {
            return Ok(0);
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&orphaned_file_vectorization_requeue_sql(
            now_ms, limit, paths,
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(candidate_count)
    }

    pub fn fetch_file_vectorization_queue_counts(&self) -> Result<(usize, usize)> {
        if !AxonRuntimeMode::from_env().semantic_workers_enabled() {
            return Ok((0, 0));
        }
        let queued = self.query_count_writer(
            "SELECT count(*) FROM FileVectorizationQueue WHERE status IN ('queued', 'paused_for_interactive_priority')",
        )?;
        let inflight = self.query_count_writer(
            "SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'",
        )?;
        let queued = usize::try_from(queued).unwrap_or(0);
        let inflight = usize::try_from(inflight).unwrap_or(0);
        Ok((queued, inflight))
    }

    pub fn fetch_claimable_file_vectorization_queue_count(&self) -> Result<usize> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let claimable = self.query_count_writer(&format!(
            "SELECT count(*) FROM ({}) claimable_now",
            Self::claimable_file_vectorization_candidates_query(now_ms)
        ))?;
        Ok(usize::try_from(claimable).unwrap_or(0))
    }
}
