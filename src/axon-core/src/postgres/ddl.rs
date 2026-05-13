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

/// Global DDL: extensions + public registry + soll intent layer + IST
/// multi-project tables (post-CPT-AXO-039 supersedure 2026-05-08) +
/// axon_runtime indexer telemetry. Stable, byte-identical across calls
/// for the same Axon binary build.
pub fn generate_global_schema() -> Vec<String> {
    let mut stmts: Vec<String> = vec![
        // Extensions. Must come first; both are required for the rest.
        "CREATE EXTENSION IF NOT EXISTS age".to_string(),
        "CREATE EXTENSION IF NOT EXISTS vector".to_string(),
        // SOLL schema: shared intent layer across all projects.
        // Created BEFORE soll.ProjectCodeRegistry so the table can land in
        // its canonical schema. CPT-AXO-038: client owns this — Axon just
        // populates it via axon_init_project.
        "CREATE SCHEMA IF NOT EXISTS soll".to_string(),
        // Project registry — REQ-AXO-247: must live in `soll` (not
        // `public`) to match the consumer code path that the DuckDB-era
        // init_schema established (graph_bootstrap.rs:1368). Columns
        // mirror the DuckDB ALTER chain (project_name, project_path,
        // project_slug, session_pointer_json) so axon_init_project +
        // soll_validate + axon_commit_work all round-trip on PG.
        "CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (\
            project_code TEXT PRIMARY KEY,\
            project_name TEXT,\
            project_path TEXT,\
            project_slug TEXT,\
            session_pointer_json TEXT,\
            registered_at_ms BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT\
         )"
        .to_string(),
        "CREATE UNIQUE INDEX IF NOT EXISTS soll_project_code_registry_code_idx ON soll.ProjectCodeRegistry(project_code)"
            .to_string(),
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
            project_code TEXT NOT NULL DEFAULT '',\
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
            project_code TEXT NOT NULL DEFAULT '',\
            metadata JSONB,\
            PRIMARY KEY (source_id, target_id, relation_type)\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.Revision (\
            revision_id TEXT PRIMARY KEY,\
            project_code TEXT NOT NULL DEFAULT '',\
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
            project_code TEXT NOT NULL DEFAULT '',\
            action TEXT NOT NULL,\
            before_json JSONB,\
            after_json JSONB,\
            created_at BIGINT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS soll.RevisionPreview (\
            preview_id TEXT PRIMARY KEY,\
            author TEXT,\
            project_code TEXT NOT NULL DEFAULT '',\
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
        // REQ-AXO-247 — McpJob mirror of DuckDB-era init_schema:1385.
        // axon_commit_work + soll_apply_plan persist async-job state
        // here; without it those tools fail under PG.
        "CREATE TABLE IF NOT EXISTS soll.McpJob (\
            job_id TEXT PRIMARY KEY,\
            tool_name TEXT,\
            status TEXT,\
            submitted_at BIGINT,\
            started_at BIGINT,\
            finished_at BIGINT,\
            request_json JSONB,\
            reserved_ids_json JSONB,\
            result_json JSONB,\
            error_text TEXT,\
            project_code TEXT\
         )"
        .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_mcp_job_status_idx ON soll.McpJob (status, submitted_at)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS soll_mcp_job_project_idx ON soll.McpJob (project_code, status)"
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
    ];
    // Append the multi-project IST layer (CPT-AXO-039 superseded by
    // multi-project tables, 2026-05-08).
    stmts.extend(ist_ddl_global());
    stmts
}

/// Multi-project IST DDL (post-CPT-AXO-039 supersedure 2026-05-08).
/// Every IST table lives in `public` with a `project_code` column to
/// scope rows. This mirrors the DuckDB layout and means the PG migration
/// is purely a SQL-dialect swap (INSERT OR REPLACE → ON CONFLICT,
/// FLOAT[N] → vector(N), array_cosine_distance → `<=>`) rather than a
/// schema-namespacing refactor. Cross-project queries become a simple
/// `WHERE project_code IN (...)` instead of `UNION ALL` across schemas.
fn ist_ddl_global() -> Vec<String> {
    let dim = DIMENSION;
    vec![
        // ── Core IST tables ────────────────────────────────────────
        "CREATE TABLE IF NOT EXISTS public.File (\
            path TEXT PRIMARY KEY,\
            project_code TEXT NOT NULL DEFAULT '',\
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
        .to_string(),
        // REQ-AXO-289 streaming pipeline v2 — minimal watcher filter table.
        // 3 columns only: path PK, content_hash for change detection,
        // last_seen_ms for hygiene. NO status machine, NO worker_id, NO
        // claim state. Replaces public.File during the v2 cut-over
        // (slice S7-S8). Until then the two coexist; v2 stages exclusively
        // read+write IndexedFile, legacy ingestion still writes public.File.
        "CREATE TABLE IF NOT EXISTS public.IndexedFile (\
            path TEXT PRIMARY KEY,\
            content_hash TEXT NOT NULL,\
            last_seen_ms BIGINT NOT NULL\
         )"
        .to_string(),
        format!(
            "CREATE TABLE IF NOT EXISTS public.Symbol (\
                id TEXT PRIMARY KEY,\
                name TEXT NOT NULL,\
                kind TEXT,\
                tested BOOLEAN NOT NULL DEFAULT FALSE,\
                is_public BOOLEAN NOT NULL DEFAULT FALSE,\
                is_nif BOOLEAN NOT NULL DEFAULT FALSE,\
                is_unsafe BOOLEAN NOT NULL DEFAULT FALSE,\
                project_code TEXT NOT NULL DEFAULT '',\
                embedding vector({dim})\
             )"
        ),
        "CREATE TABLE IF NOT EXISTS public.Chunk (\
            id TEXT PRIMARY KEY,\
            source_type TEXT,\
            source_id TEXT,\
            project_code TEXT NOT NULL DEFAULT '',\
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
        .to_string(),
        // REQ-AXO-292 — FTS GENERATED column + GIN index. The weights
        // mirror the hybrid retrieval plan (chunk_path / kind as title
        // = A, content body = B, file_path as path metadata = C).
        // GENERATED ALWAYS AS STORED means PG recomputes it on every
        // INSERT/UPDATE of `content` (or chunk_path / kind / file_path).
        // Adds ~0.5 ms / chunk on write but unlocks the lexical lane
        // for hybrid retrieval (gate ≥ 250 ch/s sustained).
        "ALTER TABLE public.Chunk \
         ADD COLUMN IF NOT EXISTS content_tsv tsvector \
         GENERATED ALWAYS AS ( \
             setweight(to_tsvector('simple', coalesce(chunk_path, '')), 'A') || \
             setweight(to_tsvector('simple', coalesce(kind, '')), 'A') || \
             setweight(to_tsvector('english', coalesce(content, '')), 'B') || \
             setweight(to_tsvector('simple', coalesce(file_path, '')), 'C') \
         ) STORED"
        .to_string(),
        "CREATE INDEX IF NOT EXISTS idx_chunk_content_tsv \
         ON public.Chunk USING GIN(content_tsv)"
        .to_string(),
        "CREATE INDEX IF NOT EXISTS idx_chunk_project_code \
         ON public.Chunk(project_code)"
        .to_string(),
        format!(
            "CREATE TABLE IF NOT EXISTS public.ChunkEmbedding (\
                chunk_id TEXT NOT NULL,\
                model_id TEXT NOT NULL,\
                project_code TEXT NOT NULL DEFAULT '',\
                source_hash TEXT NOT NULL,\
                embedding vector({dim}) NOT NULL,\
                embedded_at_ms BIGINT NOT NULL,\
                PRIMARY KEY (chunk_id, model_id)\
             )"
        ),
        // ── Relation tables — REINTRODUCED (MIL-AXO-017 slice 1) ──
        // REQ-AXO-216 (Stop A) dropped the 5 per-type SQL relation
        // tables in favor of AGE elabels (axon_graph). REQ-AXO-295
        // (DEC-AXO-083) reintroduces a single unified `public.Edge`
        // table backed by composite B-tree + GIN metadata indexes
        // because AGE was 3-5× slower at depth=5 traversal due to
        // agtype encode/decode overhead and absence of indexes on
        // create_elabel() tables. Schema-only backup preserved at
        // /home/dstadel/backups/pg/relations-schema-pre-stopA-
        // 20260509T215841Z.sql (pre-Stop-A snapshot for audit).
        "CREATE TABLE IF NOT EXISTS public.Edge (\
            source_id     TEXT NOT NULL,\
            target_id     TEXT NOT NULL,\
            relation_type TEXT NOT NULL,\
            project_code  TEXT NOT NULL DEFAULT '',\
            metadata      JSONB,\
            created_at_ms BIGINT NOT NULL,\
            PRIMARY KEY (source_id, target_id, relation_type, project_code)\
         )"
        .to_string(),
        "CREATE INDEX IF NOT EXISTS edge_fwd_idx \
            ON public.Edge (source_id, relation_type, target_id)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS edge_rev_idx \
            ON public.Edge (target_id, relation_type, source_id)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS edge_proj_idx \
            ON public.Edge (project_code, relation_type)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS edge_metadata_idx \
            ON public.Edge USING GIN (metadata jsonb_path_ops)"
            .to_string(),
        // ── Queues ────────────────────────────────────────────────
        "CREATE TABLE IF NOT EXISTS public.FileVectorizationQueue (\
            file_path TEXT PRIMARY KEY,\
            project_code TEXT NOT NULL DEFAULT '',\
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
        .to_string(),
        "CREATE TABLE IF NOT EXISTS public.GraphProjectionQueue (\
            anchor_type TEXT NOT NULL,\
            anchor_id TEXT NOT NULL,\
            radius BIGINT NOT NULL,\
            project_code TEXT NOT NULL DEFAULT '',\
            status TEXT NOT NULL DEFAULT 'queued',\
            attempts BIGINT NOT NULL DEFAULT 0,\
            queued_at BIGINT,\
            last_error_reason TEXT,\
            last_attempt_at BIGINT,\
            PRIMARY KEY (anchor_type, anchor_id, radius)\
         )"
        .to_string(),
        // ── Tables consumed by the indexer hot path that the
        //    DuckDB schema also creates. Listed here so the PG-backed
        //    indexer (REQ-AXO-242) can boot without DDL gaps.
        // ──────────────────────────────────────────────────────────
        // Project: ingress promoter writes one row per project_code
        // (`INSERT INTO Project (name) ... ON CONFLICT DO NOTHING`).
        // Mirrors the DuckDB shape `Project (name VARCHAR PRIMARY KEY)`.
        "CREATE TABLE IF NOT EXISTS public.Project (\
            name TEXT PRIMARY KEY\
         )"
        .to_string(),
        // EmbeddingModel: one row per (id, kind) registered embedding
        // model. Indexer writes on first vector-lane init.
        "CREATE TABLE IF NOT EXISTS public.EmbeddingModel (\
            id TEXT PRIMARY KEY,\
            kind TEXT,\
            model_name TEXT,\
            dimension BIGINT,\
            version TEXT,\
            created_at BIGINT\
         )"
        .to_string(),
        // GraphProjection / GraphProjectionState: cache of derived
        // graph traversals keyed by (anchor_type, anchor_id, radius).
        "CREATE TABLE IF NOT EXISTS public.GraphProjection (\
            anchor_type TEXT NOT NULL,\
            anchor_id TEXT NOT NULL,\
            target_type TEXT,\
            target_id TEXT,\
            edge_kind TEXT,\
            distance BIGINT,\
            radius BIGINT NOT NULL,\
            project_code TEXT NOT NULL DEFAULT '',\
            projection_version TEXT,\
            created_at BIGINT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS public.GraphProjectionState (\
            anchor_type TEXT NOT NULL,\
            anchor_id TEXT NOT NULL,\
            radius BIGINT NOT NULL,\
            project_code TEXT NOT NULL DEFAULT '',\
            source_signature TEXT,\
            projection_version TEXT,\
            updated_at BIGINT,\
            PRIMARY KEY (anchor_type, anchor_id, radius, project_code)\
         )"
        .to_string(),
        // GraphEmbedding: vectorised cache of graph traversals. Same
        // dimension as ChunkEmbedding because both use the BGE model.
        format!(
            "CREATE TABLE IF NOT EXISTS public.GraphEmbedding (\
                anchor_type TEXT NOT NULL,\
                anchor_id TEXT NOT NULL,\
                radius BIGINT NOT NULL,\
                model_id TEXT NOT NULL,\
                project_code TEXT NOT NULL DEFAULT '',\
                source_signature TEXT,\
                projection_version TEXT,\
                embedding vector({dim}),\
                updated_at BIGINT,\
                PRIMARY KEY (anchor_type, anchor_id, radius, model_id, project_code)\
             )"
        ),
        // RewardObservationLog: throughput-vs-decision telemetry,
        // written by the optimiser feedback loop.
        "CREATE TABLE IF NOT EXISTS public.RewardObservationLog (\
            decision_id TEXT NOT NULL,\
            observed_at_ms BIGINT,\
            window_start_ms BIGINT,\
            window_end_ms BIGINT,\
            reward_json TEXT,\
            throughput_chunks_per_hour DOUBLE PRECISION,\
            throughput_files_per_hour DOUBLE PRECISION,\
            constraint_violations_json TEXT,\
            pressure_summary_json TEXT\
         )"
        .to_string(),
        // ── Telemetry / lifecycle ─────────────────────────────────
        "CREATE TABLE IF NOT EXISTS public.FileLifecycleEvent (\
            file_path TEXT NOT NULL,\
            project_code TEXT NOT NULL DEFAULT '',\
            stage TEXT NOT NULL,\
            status TEXT NOT NULL,\
            reason TEXT,\
            at_ms BIGINT NOT NULL,\
            worker_id BIGINT,\
            trace_id TEXT,\
            run_id TEXT\
         )"
        .to_string(),
        "CREATE TABLE IF NOT EXISTS public.HourlyVectorizationRollup (\
            bucket_start_ms BIGINT NOT NULL,\
            project_code TEXT NOT NULL DEFAULT '',\
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
        .to_string(),
        // ── Indexes (note: project_code is part of every hot filter,
        //    so it leads composite indexes where available) ─────────
        "CREATE INDEX IF NOT EXISTS file_project_status_idx ON public.File (project_code, status)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS file_project_stage_ready_idx ON public.File (project_code, file_stage, graph_ready, vector_ready)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS symbol_project_kind_idx ON public.Symbol (project_code, kind)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS symbol_project_name_idx ON public.Symbol (project_code, name)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS chunk_project_source_idx ON public.Chunk (project_code, source_type, source_id)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS chunk_project_file_idx ON public.Chunk (project_code, file_path)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS chunk_embedding_project_idx ON public.ChunkEmbedding (project_code)"
            .to_string(),
        // REQ-AXO-216 / Stop A: relation table indexes removed alongside
        // the tables themselves (CONTAINS / CALLS / CALLS_NIF / IMPACTS).
        // AGE elabels carry their own internal indexing.
        "CREATE INDEX IF NOT EXISTS file_vec_queue_project_status_idx ON public.FileVectorizationQueue (project_code, status, queued_at)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS gp_queue_project_status_idx ON public.GraphProjectionQueue (project_code, status, queued_at)"
            .to_string(),
        "CREATE INDEX IF NOT EXISTS file_lifecycle_project_at_idx ON public.FileLifecycleEvent (project_code, at_ms)"
            .to_string(),
        // ── pgvector HNSW (CPT-AXO-041) ──────────────────────────
        // Single global index covers all projects; the project_code
        // filter is applied via WHERE clause and pgvector's iterative
        // scan handles the post-filter efficiently.
        "CREATE INDEX IF NOT EXISTS chunk_embedding_hnsw_idx ON public.ChunkEmbedding USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64)"
            .to_string(),
        // ── AGE graph namespace (CPT-AXO-040 expanded for option B) ─
        // Single global graph hosting structural edges. Vertices for
        // File / Symbol / Chunk are mirrored from the SQL tables (which
        // remain authoritative for indexed attribute lookups + pgvector
        // ANN). Edges (CONTAINS / CALLS / CALLS_NIF / IMPACTS /
        // SUBSTANTIATES) are progressively migrated from SQL relation
        // tables into AGE elabels (option B roadmap).
        //
        // For phase B.1 (DDL only) we declare every label up-front so
        // future writer slices can `CREATE (n:Symbol ...)` /
        // `MATCH ()-[:CONTAINS]->()` without DDL drift.
        "DO $$\n\
         BEGIN\n\
           IF NOT EXISTS (SELECT 1 FROM ag_catalog.ag_graph WHERE name = 'axon_graph') THEN\n\
             PERFORM create_graph('axon_graph');\n\
           END IF;\n\
         END\n\
         $$"
        .to_string(),
        // Vertex labels mirrored from SQL entity tables. AGE rejects
        // CREATE on a label that already exists; wrap in DO/EXCEPTION
        // for idempotence.
        age_idempotent_create("vlabel", "File").to_string(),
        age_idempotent_create("vlabel", "Symbol").to_string(),
        age_idempotent_create("vlabel", "Chunk").to_string(),
        // Edge labels — destinations of phase B.2 writer migration.
        age_idempotent_create("elabel", "CONTAINS").to_string(),
        age_idempotent_create("elabel", "CALLS").to_string(),
        age_idempotent_create("elabel", "CALLS_NIF").to_string(),
        age_idempotent_create("elabel", "IMPACTS").to_string(),
        age_idempotent_create("elabel", "SUBSTANTIATES").to_string(),
    ]
}

/// Compose an idempotent AGE label-creation statement. AGE's
/// `create_vlabel` / `create_elabel` raise when the label already
/// exists, but the exception they raise is `XX000` (internal_error)
/// with a free-text message ("label 'X' already exists"), not the
/// SQL-standard `42P07` (duplicate_table) or `42710`
/// (duplicate_object) we would expect. The narrow handlers were
/// silently letting the second-run schema bootstrap fail under
/// `bootstrap_global_pg_schema`. The handler now also catches the
/// catch-all `OTHERS` branch so the operation is fully idempotent —
/// safe because the function only ever calls one PERFORM with a
/// hardcoded label, so any thrown exception either means
/// "already exists" (the desired no-op) or a real DDL bug that the
/// upstream test suite + smoke tests will surface.
fn age_idempotent_create(kind: &'static str, label: &str) -> String {
    let func = match kind {
        "vlabel" => "create_vlabel",
        "elabel" => "create_elabel",
        _ => unreachable!("invalid AGE label kind"),
    };
    format!(
        "DO $$\n\
         BEGIN\n\
           PERFORM {func}('axon_graph', '{label}');\n\
         EXCEPTION\n\
           WHEN duplicate_table THEN NULL;\n\
           WHEN duplicate_object THEN NULL;\n\
           WHEN sqlstate '42P07' THEN NULL;\n\
           WHEN OTHERS THEN \n\
             IF SQLERRM LIKE '%already exists%' THEN \n\
               NULL; \n\
             ELSE \n\
               RAISE; \n\
             END IF;\n\
         END\n\
         $$"
    )
}

/// Per-project provisioning entry point.
///
/// Pre-supersedure (CPT-AXO-039 era) this function created a dedicated
/// PG schema per project. Post-supersedure (2026-05-08) it's a thin
/// pass-through that just validates the project_code and returns an
/// empty plan: every IST table now lives in `public` with a
/// `project_code` column, provisioned once by `generate_global_schema`.
/// We keep the function for API stability — `axon_init_project` still
/// calls it, and it still rejects malformed codes (SQL-injection guard
/// applies even if no DDL fires).
pub fn generate_project_schema(project_code: &str) -> Result<Vec<String>> {
    // Validate the project_code shape — same guard as before so
    // callers get the same error semantics on bad input.
    let _ = schema_name_for(project_code)?;
    Ok(Vec::new())
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
    fn project_schema_is_now_no_op() {
        // CPT-AXO-039 superseded 2026-05-08: per-project schema replaced
        // by multi-project tables in `public`. The function still
        // validates project_code shape but emits zero DDL statements.
        let stmts = generate_project_schema("AXO").unwrap();
        assert!(
            stmts.is_empty(),
            "generate_project_schema should be a no-op post-CPT-AXO-039 supersedure"
        );
    }

    #[test]
    fn global_schema_includes_required_objects() {
        let stmts = generate_global_schema();
        let joined = stmts.join("\n");
        assert!(joined.contains("CREATE EXTENSION IF NOT EXISTS age"));
        assert!(joined.contains("CREATE EXTENSION IF NOT EXISTS vector"));
        assert!(joined.contains("CREATE SCHEMA IF NOT EXISTS soll"));
        // REQ-AXO-247: ProjectCodeRegistry now lives in `soll`, not
        // `public`, so the consumer code path (axon_init_project,
        // soll_validate, axon_commit_work) finds it under PG.
        assert!(joined.contains("soll.ProjectCodeRegistry"));
        assert!(!joined.contains("public.ProjectCodeRegistry"),
            "PCR should no longer be in public; consumers query soll.*");
        assert!(joined.contains("soll_project_code_registry_code_idx"));
        for tbl in [
            "soll.Registry",
            "soll.Node",
            "soll.Edge",
            "soll.Revision",
            "soll.RevisionChange",
            "soll.RevisionPreview",
            "soll.Traceability",
            "soll.McpJob",
        ] {
            assert!(
                joined.contains(tbl),
                "expected SOLL schema to contain {tbl}"
            );
        }
    }

    #[test]
    fn age_idempotent_create_catches_already_exists_message() {
        // Regression for the bench-blocker discovered 2026-05-08:
        // AGE's create_vlabel raises sqlstate 'XX000' (internal_error)
        // with message 'label "File" already exists' — the narrow
        // duplicate_table / duplicate_object handlers don't catch it,
        // so a second-run bootstrap_global_pg_schema fails. The fix
        // adds an OTHERS branch that no-ops only when SQLERRM matches
        // 'already exists', re-raising every other error.
        let stmt = age_idempotent_create("vlabel", "File");
        assert!(stmt.contains("WHEN OTHERS THEN"));
        assert!(stmt.contains("SQLERRM LIKE '%already exists%'"));
        assert!(stmt.contains("RAISE"));
        // Specific catches preserved so the common-case sqlstate match
        // still hits the cheap path.
        assert!(stmt.contains("WHEN duplicate_table THEN NULL"));
        assert!(stmt.contains("WHEN duplicate_object THEN NULL"));
    }

    #[test]
    fn global_schema_declares_age_labels_for_option_b() {
        // Option B (AGE-native edges): every relation gets a pre-
        // declared elabel and the entity vertex labels exist so the
        // writer migration (phase B.2+) can `CREATE` and `MATCH`
        // without DDL drift.
        let joined = generate_global_schema().join("\n");
        for label in ["File", "Symbol", "Chunk"] {
            assert!(
                joined.contains(&format!("create_vlabel('axon_graph', '{label}')")),
                "expected vlabel '{label}' declaration"
            );
        }
        for label in ["CONTAINS", "CALLS", "CALLS_NIF", "IMPACTS", "SUBSTANTIATES"] {
            assert!(
                joined.contains(&format!("create_elabel('axon_graph', '{label}')")),
                "expected elabel '{label}' declaration"
            );
        }
        // Idempotence guard: every label create wraps PERFORM in a DO
        // block with EXCEPTION handlers so re-running is safe.
        assert!(joined.contains("WHEN duplicate_table THEN NULL"));
        assert!(joined.contains("WHEN sqlstate '42P07' THEN NULL"));
    }

    #[test]
    fn global_schema_includes_multi_project_ist_tables() {
        // Post-CPT-AXO-039 supersedure: every IST table lives in
        // `public` with project_code as a row-level discriminator.
        // REQ-AXO-216 / Stop A: the 5 relation tables (CONTAINS /
        // CALLS / CALLS_NIF / IMPACTS / SUBSTANTIATES) were dropped
        // wave 9; AGE elabels are now canonical for edges.
        let joined = generate_global_schema().join("\n");
        for tbl in [
            "public.File",
            "public.Symbol",
            "public.Chunk",
            "public.ChunkEmbedding",
            "public.FileVectorizationQueue",
            "public.GraphProjectionQueue",
            "public.FileLifecycleEvent",
            "public.HourlyVectorizationRollup",
            // REQ-AXO-242: indexer hot-path tables added to close the
            // P9 DDL gap so axon-indexer can boot under PG.
            "public.Project",
            "public.EmbeddingModel",
            "public.GraphProjection",
            "public.GraphProjectionState",
            "public.GraphEmbedding",
            "public.RewardObservationLog",
        ] {
            assert!(
                joined.contains(tbl),
                "expected IST table {tbl} in global schema"
            );
        }
        // ChunkEmbedding gains project_code column for multi-project
        // filtering under the single global HNSW index.
        assert!(
            joined.contains("public.ChunkEmbedding")
                && joined.contains("project_code TEXT NOT NULL")
        );
        // Single global HNSW + single global AGE graph.
        assert!(
            joined.contains("CREATE INDEX IF NOT EXISTS chunk_embedding_hnsw_idx ON public.ChunkEmbedding")
        );
        assert!(joined.contains("create_graph('axon_graph')"));
        // No per-project schema artefacts left (with word boundaries
        // so `axon_runtime` doesn't trigger the false-positive).
        assert!(!joined.contains("CREATE SCHEMA IF NOT EXISTS axo "));
        assert!(!joined.contains("CREATE SCHEMA IF NOT EXISTS axo\n"));
        assert!(!joined.contains("axo.File"));
        assert!(!joined.contains("axo.Chunk"));
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
    fn project_schema_validates_input() {
        // CPT-AXO-039 superseded but the validation remains: callers
        // pass project_code through schema_name_for to reject injection
        // attempts, even though no DDL is emitted.
        assert!(generate_project_schema("axo;DROP TABLE Node").is_err());
        assert!(generate_project_schema("").is_err());
        assert!(generate_project_schema("AXO").is_ok());
    }
}
