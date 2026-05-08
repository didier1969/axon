// MIL-AXO-015 P2 (REQ-AXO-208): Per-project schema namespace generator.
//
// Two surfaces:
//   - `generate_global_schema()`: idempotent DDL for the public + soll
//     schemas (extensions, ProjectCodeRegistry, SOLL Node/Edge/Revision/
//     Traceability). Run once at deployment bootstrap.
//   - `generate_project_schema(project_code)`: idempotent DDL for one
//     project's IST namespace (File, Symbol, Chunk, ChunkEmbedding with
//     pgvector, CONTAINS/CALLS/etc. relations, queues, telemetry, AGE
//     graph). Run by axon_init_project (P5) when registering a new
//     project.
//
// Architecture references:
//   - DEC-AXO-075: PG replaces DuckDB.
//   - CPT-AXO-039: per-project schema namespace.
//   - CPT-AXO-040: Apache AGE for graph queries.
//   - CPT-AXO-041: pgvector HNSW for ChunkEmbedding.
//
// Idempotence is the design constraint: every statement uses
// IF NOT EXISTS / IF EXISTS / OR REPLACE so re-running on a healthy
// database is a no-op. P3 will exercise these against a real PG via
// testcontainers; P2 only proves DDL stability.

use anyhow::{anyhow, Result};

use crate::embedding_contract::DIMENSION;

/// Validate a project_code so it can be used as a PostgreSQL schema
/// identifier without quoting. Axon uses 3-letter uppercase codes (AXO,
/// FSF, etc.) but the schema namespace is lowercased to match Postgres
/// case-folding rules. We refuse anything that isn't strictly alphanum
/// + underscore so generated SQL is injection-free even if a malicious
/// caller bypasses the registry layer.
pub fn schema_name_for(project_code: &str) -> Result<String> {
    if project_code.is_empty() {
        return Err(anyhow!("project_code is empty"));
    }
    if project_code.len() > 32 {
        return Err(anyhow!(
            "project_code '{}' too long (>32 chars)",
            project_code
        ));
    }
    if !project_code
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(anyhow!(
            "project_code '{}' contains characters that are not [a-zA-Z0-9_]",
            project_code
        ));
    }
    Ok(project_code.to_ascii_lowercase())
}

/// Global DDL: extensions + public registry + soll intent layer.
/// Stable, byte-identical across calls for the same Axon binary build.
pub fn generate_global_schema() -> Vec<String> {
    vec![
        // Extensions. Must come first; both are required for the rest.
        "CREATE EXTENSION IF NOT EXISTS age".to_string(),
        "CREATE EXTENSION IF NOT EXISTS vector".to_string(),
        // Project registry. CPT-AXO-038: client owns this — Axon just
        // populates it via axon_init_project.
        "CREATE TABLE IF NOT EXISTS public.ProjectCodeRegistry (\
            project_code TEXT PRIMARY KEY,\
            project_name TEXT NOT NULL,\
            project_path TEXT NOT NULL,\
            registered_at_ms BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT\
         )"
        .to_string(),
        // SOLL schema: shared intent layer across all projects.
        "CREATE SCHEMA IF NOT EXISTS soll".to_string(),
        "CREATE TABLE IF NOT EXISTS soll.Registry (\
            project_code TEXT PRIMARY KEY DEFAULT 'AXON_GLOBAL',\
            id TEXT NOT NULL DEFAULT 'AXON_GLOBAL',\
            last_vis BIGINT NOT NULL DEFAULT 0,\
            last_pil BIGINT NOT NULL DEFAULT 0,\
            last_req BIGINT NOT NULL DEFAULT 0,\
            last_cpt BIGINT NOT NULL DEFAULT 0,\
            last_dec BIGINT NOT NULL DEFAULT 0,\
            last_mil BIGINT NOT NULL DEFAULT 0,\
            last_val BIGINT NOT NULL DEFAULT 0,\
            last_stk BIGINT NOT NULL DEFAULT 0,\
            last_gui BIGINT NOT NULL DEFAULT 0,\
            last_prv BIGINT NOT NULL DEFAULT 0,\
            last_rev BIGINT NOT NULL DEFAULT 0\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.Node (\
            id TEXT PRIMARY KEY,\
            type TEXT NOT NULL,\
            project_code TEXT NOT NULL,\
            title TEXT,\
            description TEXT,\
            status TEXT,\
            metadata JSONB\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.Edge (\
            source_id TEXT NOT NULL,\
            target_id TEXT NOT NULL,\
            relation_type TEXT NOT NULL,\
            project_code TEXT NOT NULL,\
            metadata JSONB,\
            PRIMARY KEY (source_id, target_id, relation_type)\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.Revision (\
            revision_id TEXT PRIMARY KEY,\
            project_code TEXT NOT NULL,\
            author TEXT,\
            source TEXT,\
            summary TEXT,\
            status TEXT,\
            created_at BIGINT,\
            committed_at BIGINT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.RevisionChange (\
            revision_id TEXT NOT NULL,\
            entity_type TEXT NOT NULL,\
            entity_id TEXT NOT NULL,\
            project_code TEXT NOT NULL,\
            action TEXT NOT NULL,\
            before_json JSONB,\
            after_json JSONB,\
            created_at BIGINT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.RevisionPreview (\
            preview_id TEXT PRIMARY KEY,\
            author TEXT,\
            project_code TEXT NOT NULL,\
            payload JSONB,\
            created_at BIGINT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.Traceability (\
            id TEXT PRIMARY KEY,\
            soll_entity_type TEXT NOT NULL,\
            soll_entity_id TEXT NOT NULL,\
            artifact_type TEXT NOT NULL,\
            artifact_ref TEXT NOT NULL,\
            confidence DOUBLE PRECISION,\
            metadata JSONB,\
            created_at BIGINT\
         )"
        .to_string(),
        // Indexes for hot SOLL multi-tenant lookups.
        "CREATE INDEX IF NOT EXISTS soll_node_project_idx ON soll.Node (project_code, type)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_node_status_idx ON soll.Node (status) WHERE status IS NOT NULL"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_edge_project_source_idx ON soll.Edge (project_code, source_id)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_edge_project_target_idx ON soll.Edge (project_code, target_id)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_revision_project_idx ON soll.Revision (project_code, created_at)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_revision_change_project_idx ON soll.RevisionChange (revision_id)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_traceability_entity_idx ON soll.Traceability (soll_entity_id, soll_entity_type)"
            .to_string(),
        // ── Indexer runtime layer (MIL-AXO-015 P4 4e seed) ────────
        // Tables consumed by the indexer hot path that don't belong to
        // a single project's IST namespace. Kept in `axon_runtime` so
        // they're isolated from SOLL and the per-project schemas.
        "CREATE SCHEMA IF NOT EXISTS axon_runtime".to_string(),
        "CREATE TABLE IF NOT EXISTS axon_runtime.OptimizerDecisionLog (\
            decision_id TEXT PRIMARY KEY,\
            at_ms BIGINT,\
            mode TEXT,\
            host_snapshot_json TEXT,\
            policy_snapshot_json TEXT,\
            signal_snapshot_json TEXT,\
            analytics_snapshot_json TEXT,\
            action_profile_id TEXT,\
            decision_json TEXT,\
            constraints_triggered_json TEXT,\
            would_apply BOOLEAN,\
            applied BOOLEAN,\
            evaluation_window_start_ms BIGINT,\
            evaluation_window_end_ms BIGINT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS axon_runtime.VectorWorkerFault (\
            fault_id TEXT PRIMARY KEY,\
            lane TEXT,\
            worker_id BIGINT,\
            fatal_stage TEXT,\
            fatal_reason_raw TEXT,\
            fatal_class TEXT,\
            provider TEXT,\
            batch_id TEXT,\
            texts_count BIGINT NOT NULL DEFAULT 0,\
            input_bytes BIGINT NOT NULL DEFAULT 0,\
            vram_used_mb BIGINT NOT NULL DEFAULT 0,\
            occurred_at_ms BIGINT,\
            restart_attempt BIGINT NOT NULL DEFAULT 0\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS axon_runtime.VectorLaneState (\
            lane TEXT PRIMARY KEY,\
            state TEXT,\
            reason TEXT,\
            updated_at_ms BIGINT,\
            worker_id BIGINT,\
            restart_attempt BIGINT NOT NULL DEFAULT 0,\
            last_success_at_ms BIGINT,\
            last_fault_id TEXT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS axon_runtime.VectorPersistOutbox (\
            outbox_id TEXT PRIMARY KEY,\
            run_id TEXT,\
            model_id TEXT,\
            status TEXT NOT NULL DEFAULT 'queued',\
            attempts BIGINT NOT NULL DEFAULT 0,\
            queued_at_ms BIGINT,\
            claimed_at_ms BIGINT,\
            completed_at_ms BIGINT,\
            last_error_reason TEXT,\
            claim_token TEXT,\
            lease_heartbeat_at_ms BIGINT,\
            lease_owner TEXT,\
            lease_epoch BIGINT NOT NULL DEFAULT 0,\
            chunk_count BIGINT NOT NULL DEFAULT 0,\
            file_count BIGINT NOT NULL DEFAULT 0,\
            input_bytes BIGINT NOT NULL DEFAULT 0,\
            fetch_ms BIGINT NOT NULL DEFAULT 0,\
            embed_ms BIGINT NOT NULL DEFAULT 0,\
            payload_json TEXT\
         )"
        .to_string(),
        "CREATE INDEX IF NOT EXISTS vector_persist_outbox_status_idx ON axon_runtime.VectorPersistOutbox (status, queued_at_ms)"
            .to_string(),
        // vector_batch_run carries dev-bench telemetry. Lower-case
        // identifier matches the DuckDB definition for grep continuity.
        "CREATE TABLE IF NOT EXISTS axon_runtime.vector_batch_run (\
            run_id TEXT PRIMARY KEY,\
            prepare_started_at_ms BIGINT NOT NULL DEFAULT 0,\
            prepare_finished_at_ms BIGINT NOT NULL DEFAULT 0,\
            ready_enqueued_at_ms BIGINT NOT NULL DEFAULT 0,\
            started_at_ms BIGINT NOT NULL,\
            finished_at_ms BIGINT NOT NULL,\
            gpu_started_at_ms BIGINT NOT NULL DEFAULT 0,\
            gpu_finished_at_ms BIGINT NOT NULL DEFAULT 0,\
            persist_enqueued_at_ms BIGINT NOT NULL DEFAULT 0,\
            persist_started_at_ms BIGINT NOT NULL DEFAULT 0,\
            persist_finished_at_ms BIGINT NOT NULL DEFAULT 0,\
            finalize_enqueued_at_ms BIGINT NOT NULL DEFAULT 0,\
            finalize_finished_at_ms BIGINT NOT NULL DEFAULT 0,\
            wall_ms BIGINT NOT NULL,\
            instance_kind TEXT NOT NULL,\
            runtime_mode TEXT NOT NULL,\
            provider TEXT NOT NULL,\
            provider_effective TEXT NOT NULL,\
            runner_kind TEXT NOT NULL DEFAULT '',\
            model_id TEXT NOT NULL,\
            vector_workers BIGINT NOT NULL,\
            graph_workers BIGINT NOT NULL,\
            ready_queue_depth BIGINT NOT NULL,\
            prepare_pipeline_depth BIGINT NOT NULL,\
            prepare_workers_per_vector BIGINT NOT NULL,\
            micro_batch_max_items BIGINT NOT NULL,\
            micro_batch_max_total_tokens BIGINT NOT NULL,\
            max_embed_batch_bytes BIGINT NOT NULL,\
            chunk_count BIGINT NOT NULL,\
            file_count BIGINT NOT NULL,\
            input_bytes BIGINT NOT NULL,\
            total_tokens BIGINT NOT NULL DEFAULT 0\
         )"
        .to_string(),
    ]
}

/// Per-project IST schema. CPT-AXO-039.
/// `project_code` must already be validated by `schema_name_for`.
pub fn generate_project_schema(project_code: &str) -> Result<Vec<String>> {
    let s = schema_name_for(project_code)?;
    let dim = DIMENSION;
    Ok(vec![
        format!("CREATE SCHEMA IF NOT EXISTS {s}"),
        // ── Core IST tables ────────────────────────────────────────
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.File (\
                path TEXT PRIMARY KEY,\
                project_code TEXT NOT NULL,\
                status TEXT,\
                size BIGINT,\
                priority BIGINT,\
                mtime BIGINT,\
                worker_id BIGINT,\
                trace_id TEXT,\
                needs_reindex BOOLEAN NOT NULL DEFAULT FALSE,\
                last_error_reason TEXT,\
                status_reason TEXT,\
                defer_count BIGINT NOT NULL DEFAULT 0,\
                last_deferred_at_ms BIGINT,\
                file_stage TEXT NOT NULL DEFAULT 'promoted',\
                graph_ready BOOLEAN NOT NULL DEFAULT FALSE,\
                vector_ready BOOLEAN NOT NULL DEFAULT FALSE,\
                first_seen_at_ms BIGINT,\
                indexing_started_at_ms BIGINT,\
                graph_ready_at_ms BIGINT,\
                vectorization_started_at_ms BIGINT,\
                vector_ready_at_ms BIGINT,\
                last_state_change_at_ms BIGINT,\
                last_error_at_ms BIGINT\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.Symbol (\
                id TEXT PRIMARY KEY,\
                name TEXT NOT NULL,\
                kind TEXT,\
                tested BOOLEAN NOT NULL DEFAULT FALSE,\
                is_public BOOLEAN NOT NULL DEFAULT FALSE,\
                is_nif BOOLEAN NOT NULL DEFAULT FALSE,\
                is_unsafe BOOLEAN NOT NULL DEFAULT FALSE,\
                project_code TEXT NOT NULL,\
                embedding vector({dim})\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.Chunk (\
                id TEXT PRIMARY KEY,\
                source_type TEXT,\
                source_id TEXT,\
                project_code TEXT NOT NULL,\
                file_path TEXT,\
                kind TEXT,\
                content TEXT,\
                content_hash TEXT,\
                start_line BIGINT,\
                end_line BIGINT,\
                chunk_part_index BIGINT,\
                chunk_part_count BIGINT,\
                chunk_path TEXT\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.ChunkEmbedding (\
                chunk_id TEXT NOT NULL,\
                model_id TEXT NOT NULL,\
                source_hash TEXT NOT NULL,\
                embedding vector({dim}) NOT NULL,\
                embedded_at_ms BIGINT NOT NULL,\
                PRIMARY KEY (chunk_id, model_id)\
             )"
        ),
        // ── Relation tables ───────────────────────────────────────
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.CONTAINS (\
                source_id TEXT NOT NULL,\
                target_id TEXT NOT NULL,\
                project_code TEXT NOT NULL,\
                PRIMARY KEY (source_id, target_id)\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.CALLS (\
                source_id TEXT NOT NULL,\
                target_id TEXT NOT NULL,\
                project_code TEXT NOT NULL,\
                PRIMARY KEY (source_id, target_id)\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.CALLS_NIF (\
                source_id TEXT NOT NULL,\
                target_id TEXT NOT NULL,\
                project_code TEXT NOT NULL,\
                PRIMARY KEY (source_id, target_id)\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.IMPACTS (\
                source_id TEXT NOT NULL,\
                target_id TEXT NOT NULL,\
                project_code TEXT NOT NULL,\
                PRIMARY KEY (source_id, target_id)\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.SUBSTANTIATES (\
                source_id TEXT NOT NULL,\
                target_id TEXT NOT NULL,\
                project_code TEXT NOT NULL,\
                PRIMARY KEY (source_id, target_id)\
             )"
        ),
        // ── Queues ────────────────────────────────────────────────
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.FileVectorizationQueue (\
                file_path TEXT PRIMARY KEY,\
                status TEXT NOT NULL DEFAULT 'queued',\
                status_reason TEXT,\
                attempts BIGINT NOT NULL DEFAULT 0,\
                queued_at BIGINT,\
                last_error_reason TEXT,\
                last_attempt_at BIGINT,\
                next_eligible_at_ms BIGINT,\
                interactive_pause_count BIGINT NOT NULL DEFAULT 0,\
                claim_token TEXT,\
                claimed_at_ms BIGINT,\
                lease_heartbeat_at_ms BIGINT,\
                lease_owner TEXT,\
                lease_epoch BIGINT NOT NULL DEFAULT 0,\
                persist_started_at_ms BIGINT\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.GraphProjectionQueue (\
                anchor_type TEXT NOT NULL,\
                anchor_id TEXT NOT NULL,\
                radius BIGINT NOT NULL,\
                status TEXT NOT NULL DEFAULT 'queued',\
                attempts BIGINT NOT NULL DEFAULT 0,\
                queued_at BIGINT,\
                last_error_reason TEXT,\
                last_attempt_at BIGINT,\
                PRIMARY KEY (anchor_type, anchor_id, radius)\
             )"
        ),
        // ── Telemetry / lifecycle ─────────────────────────────────
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.FileLifecycleEvent (\
                file_path TEXT NOT NULL,\
                project_code TEXT NOT NULL,\
                stage TEXT NOT NULL,\
                status TEXT NOT NULL,\
                reason TEXT,\
                at_ms BIGINT NOT NULL,\
                worker_id BIGINT,\
                trace_id TEXT,\
                run_id TEXT\
             )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {s}.HourlyVectorizationRollup (\
                bucket_start_ms BIGINT NOT NULL,\
                project_code TEXT NOT NULL,\
                model_id TEXT NOT NULL,\
                chunks_embedded BIGINT NOT NULL DEFAULT 0,\
                files_vector_ready BIGINT NOT NULL DEFAULT 0,\
                batches BIGINT NOT NULL DEFAULT 0,\
                fetch_ms_total BIGINT NOT NULL DEFAULT 0,\
                embed_ms_total BIGINT NOT NULL DEFAULT 0,\
                db_write_ms_total BIGINT NOT NULL DEFAULT 0,\
                mark_done_ms_total BIGINT NOT NULL DEFAULT 0,\
                PRIMARY KEY (bucket_start_ms, project_code, model_id)\
             )"
        ),
        // ── Indexes ──────────────────────────────────────────────
        format!(
            "CREATE INDEX IF NOT EXISTS file_status_idx ON {s}.File (status) WHERE status IS NOT NULL"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS file_stage_ready_idx ON {s}.File (file_stage, graph_ready, vector_ready)"
        ),
        format!("CREATE INDEX IF NOT EXISTS symbol_kind_idx ON {s}.Symbol (kind)"),
        format!("CREATE INDEX IF NOT EXISTS symbol_name_idx ON {s}.Symbol (name)"),
        format!("CREATE INDEX IF NOT EXISTS chunk_source_idx ON {s}.Chunk (source_type, source_id)"),
        format!("CREATE INDEX IF NOT EXISTS chunk_file_idx ON {s}.Chunk (file_path)"),
        format!(
            "CREATE INDEX IF NOT EXISTS contains_target_idx ON {s}.CONTAINS (target_id)"
        ),
        format!("CREATE INDEX IF NOT EXISTS calls_target_idx ON {s}.CALLS (target_id)"),
        format!("CREATE INDEX IF NOT EXISTS calls_nif_target_idx ON {s}.CALLS_NIF (target_id)"),
        format!("CREATE INDEX IF NOT EXISTS impacts_target_idx ON {s}.IMPACTS (target_id)"),
        format!(
            "CREATE INDEX IF NOT EXISTS file_vec_queue_status_idx ON {s}.FileVectorizationQueue (status, queued_at)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS gp_queue_status_idx ON {s}.GraphProjectionQueue (status, queued_at)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS file_lifecycle_event_at_idx ON {s}.FileLifecycleEvent (at_ms)"
        ),
        // ── pgvector HNSW (CPT-AXO-041) ──────────────────────────
        format!(
            "CREATE INDEX IF NOT EXISTS chunk_embedding_hnsw_idx ON {s}.ChunkEmbedding USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64)"
        ),
        // ── AGE graph namespace (CPT-AXO-040) ────────────────────
        // create_graph is idempotent via the underlying logic but we
        // wrap in a DO block so re-running doesn't error if the graph
        // already exists.
        format!(
            "DO $$\n\
             BEGIN\n\
               IF NOT EXISTS (SELECT 1 FROM ag_catalog.ag_graph WHERE name = '{s}_graph') THEN\n\
                 PERFORM create_graph('{s}_graph');\n\
               END IF;\n\
             END\n\
             $$"
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_lowercases_and_validates() {
        assert_eq!(schema_name_for("AXO").unwrap(), "axo");
        assert_eq!(schema_name_for("FSF").unwrap(), "fsf");
        assert_eq!(schema_name_for("my_project").unwrap(), "my_project");
        assert_eq!(schema_name_for("Project_42").unwrap(), "project_42");
    }

    #[test]
    fn schema_name_rejects_injection_attempts() {
        assert!(schema_name_for("").is_err());
        assert!(schema_name_for("axo; DROP TABLE Node;--").is_err());
        assert!(schema_name_for("axo--").is_err());
        assert!(schema_name_for("axo;").is_err());
        assert!(schema_name_for("axo space").is_err());
        assert!(schema_name_for("axo'").is_err());
    }

    #[test]
    fn schema_name_rejects_overlong() {
        let long = "a".repeat(33);
        assert!(schema_name_for(&long).is_err());
        let max = "a".repeat(32);
        assert!(schema_name_for(&max).is_ok());
    }

    #[test]
    fn global_schema_is_byte_stable_across_calls() {
        let a = generate_global_schema();
        let b = generate_global_schema();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn project_schema_is_byte_stable_across_calls() {
        let a = generate_project_schema("AXO").unwrap();
        let b = generate_project_schema("AXO").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn project_schema_includes_required_tables() {
        let stmts = generate_project_schema("AXO").unwrap();
        let joined = stmts.join("\n");
        for tbl in [
            "axo.File",
            "axo.Symbol",
            "axo.Chunk",
            "axo.ChunkEmbedding",
            "axo.CONTAINS",
            "axo.CALLS",
            "axo.CALLS_NIF",
            "axo.IMPACTS",
            "axo.SUBSTANTIATES",
            "axo.FileVectorizationQueue",
            "axo.GraphProjectionQueue",
            "axo.FileLifecycleEvent",
            "axo.HourlyVectorizationRollup",
        ] {
            assert!(
                joined.contains(tbl),
                "expected schema to contain {tbl}, got:\n{joined}"
            );
        }
        assert!(
            joined.contains("USING hnsw"),
            "expected pgvector HNSW index"
        );
        assert!(
            joined.contains("create_graph('axo_graph')"),
            "expected AGE graph creation"
        );
    }

    #[test]
    fn global_schema_includes_required_objects() {
        let stmts = generate_global_schema();
        let joined = stmts.join("\n");
        assert!(joined.contains("CREATE EXTENSION IF NOT EXISTS age"));
        assert!(joined.contains("CREATE EXTENSION IF NOT EXISTS vector"));
        assert!(joined.contains("public.ProjectCodeRegistry"));
        assert!(joined.contains("CREATE SCHEMA IF NOT EXISTS soll"));
        for tbl in [
            "soll.Registry",
            "soll.Node",
            "soll.Edge",
            "soll.Revision",
            "soll.RevisionChange",
            "soll.RevisionPreview",
            "soll.Traceability",
        ] {
            assert!(
                joined.contains(tbl),
                "expected SOLL schema to contain {tbl}"
            );
        }
    }

    #[test]
    fn global_schema_includes_axon_runtime_tables() {
        // MIL-AXO-015 P4 4e seed: indexer hot-path tables must exist in
        // PG so the writer can boot under `AXON_DB_BACKEND=postgres`.
        let joined = generate_global_schema().join("\n");
        assert!(joined.contains("CREATE SCHEMA IF NOT EXISTS axon_runtime"));
        for tbl in [
            "axon_runtime.OptimizerDecisionLog",
            "axon_runtime.VectorWorkerFault",
            "axon_runtime.VectorLaneState",
            "axon_runtime.VectorPersistOutbox",
            "axon_runtime.vector_batch_run",
        ] {
            assert!(
                joined.contains(tbl),
                "expected axon_runtime schema to contain {tbl}"
            );
        }
        // Idempotence: every CREATE TABLE uses IF NOT EXISTS.
        let create_table_count = joined.matches("CREATE TABLE").count();
        let if_not_exists_count = joined.matches("CREATE TABLE IF NOT EXISTS").count();
        assert_eq!(
            create_table_count, if_not_exists_count,
            "all CREATE TABLE statements must be IF NOT EXISTS for idempotence"
        );
    }

    #[test]
    fn project_schema_uses_lowercased_namespace() {
        let stmts = generate_project_schema("FSF").unwrap();
        let joined = stmts.join("\n");
        // Must use lowercased "fsf" as the schema namespace, not "FSF".
        assert!(joined.contains("CREATE SCHEMA IF NOT EXISTS fsf"));
        assert!(joined.contains("fsf.File"));
        assert!(!joined.contains("FSF.File"));
    }

    #[test]
    fn project_schema_validates_input() {
        assert!(generate_project_schema("axo;DROP TABLE Node").is_err());
        assert!(generate_project_schema("").is_err());
    }
}
