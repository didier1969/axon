use std::collections::HashMap;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use libloading::Library;
use tracing::{info, warn};

use crate::graph::{GraphStore, LatticePool};
use crate::runtime_truth_contract::RuntimeFreshnessContract;

#[allow(dead_code)]
const STARTUP_SEMANTIC_BACKFILL_FLOOR: usize = 64;

/// REQ-AXO-91562 / DEC-AXO-901594 Slice 1 — accept an explicit override so
/// per-test harnesses can target a freshly-cloned database without leaking
/// state via the global env-var chain.
///
/// Resolution priority :
///   1. Explicit `override_url` (Slice 2 test harness will pass `Some(...)`)
///   2. `AXON_INSTANCE_KIND`-specific (`AXON_DEV_DATABASE_URL` or
///      `AXON_LIVE_DATABASE_URL`)
///   3. `DATABASE_URL` fallback
fn resolve_pg_database_url_with_override(override_url: Option<&str>) -> Result<String> {
    // REQ-AXO-901881 W2 (#17) — delegate to THE canonical resolver. This was
    // one of 4 divergent copies whose drift produced the REQ-AXO-315 dev→live
    // leak; resolution now lives only in postgres::resolve_database_url.
    crate::postgres::resolve_database_url(override_url)
}

pub fn canonical_soll_db_path(db_root: &str) -> Option<PathBuf> {
    if db_root == ":memory:" {
        return None;
    }

    let mut path = PathBuf::from(db_root);
    path.push("soll.db");
    Some(path)
}

pub fn canonical_ist_db_path(db_root: &str) -> Option<PathBuf> {
    if db_root == ":memory:" {
        return None;
    }

    let mut path = PathBuf::from(db_root);
    path.push("ist.db");
    Some(path)
}

#[allow(dead_code)]
fn startup_vector_backfill_limit(
    _structural_graph_backlog_depth: usize,
    graph_ready_depth: usize,
) -> usize {
    if graph_ready_depth == 0 {
        return 0;
    }
    let startup_budget = STARTUP_SEMANTIC_BACKFILL_FLOOR;
    startup_budget.min(graph_ready_depth)
}

// REQ-AXO-901653 slice-5a: `IstCompatibilityAction` enum deleted ;
// only consumed by the deleted `ensure_runtime_compatibility` helper.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SollAccessMode {
    ReadWrite,
    ReadOnlyOrEmptySchema,
    Detached,
}

#[allow(dead_code)]
impl GraphStore {
    pub fn new(db_root: &str) -> Result<Self> {
        // Split-brain mode arg is ignored (forced to false in
        // `new_with_modes` ; PG's MVCC handles reader/writer concurrency
        // server-side post DuckDB purge — REQ-AXO-271).
        Self::new_with_modes(db_root, false, SollAccessMode::ReadWrite, None)
    }

    pub fn new_brain_reader_soll_writer(db_root: &str) -> Result<Self> {
        Self::new_with_modes(db_root, db_root != ":memory:", SollAccessMode::ReadWrite, None)
    }

    pub fn new_indexer_ist_writer_soll_reader(db_root: &str) -> Result<Self> {
        Self::new_with_modes(db_root, false, SollAccessMode::ReadOnlyOrEmptySchema, None)
    }

    pub fn new_indexer_ist_writer_without_soll(db_root: &str) -> Result<Self> {
        Self::new_with_modes(db_root, false, SollAccessMode::Detached, None)
    }

    pub fn new_indexer_ist_writer_split(db_root: &str) -> Result<Self> {
        Self::new_indexer_ist_writer_without_soll(db_root)
    }

    /// REQ-AXO-91562 / DEC-AXO-901594 Slice 1 — explicit DATABASE_URL
    /// override factory.
    ///
    /// Per-test harnesses (Slice 2 follow-up) call this with a URL pointing
    /// to a freshly-cloned database (e.g. `postgresql://...:44144/test_<uuid>`)
    /// instead of relying on the global `AXON_LIVE_DATABASE_URL` /
    /// `AXON_DEV_DATABASE_URL` env vars. This bypasses the shared-state
    /// pollution that today causes the soll_and_guidelines cluster
    /// (REQ-AXO-915 / 91560 / 91562) to fail 106/147.
    ///
    /// `db_root` is still respected for split-brain / reader-only paths
    /// where applicable. `database_url` overrides ALL env-var resolution.
    pub fn new_with_database(db_root: &str, database_url: &str) -> Result<Self> {
        Self::new_with_modes(
            db_root,
            false,
            SollAccessMode::ReadWrite,
            Some(database_url),
        )
    }

    fn new_with_modes(
        db_root: &str,
        _split_brain_mode: bool,
        soll_access_mode: SollAccessMode,
        database_url_override: Option<&str>,
    ) -> Result<Self> {
        let plugin_path = Self::find_plugin_path()?;
        let lib = Arc::new(unsafe { Library::new(&plugin_path)? });
        let symbols = unsafe { crate::graph::PluginSymbols::resolve(&lib) }?;
        let init_fn = symbols.init_fn;
        let _close_fn = symbols.close_fn;
        // PostgreSQL's MVCC handles reader/writer concurrency natively;
        // the DuckDB-era split-brain file-isolation + reader replica are
        // retired (REQ-AXO-901870). Every read shares the single writer
        // connection pool.
        info!(
            "GraphStore init modes: db_root={}, soll_access_mode={:?}",
            db_root, soll_access_mode
        );

        // Under PostgreSQL the "DB path" is a DATABASE_URL passed verbatim
        // to pg_init_db_compat. SOLL + per-project IST live inside the same
        // database via schema namespacing (CPT-AXO-039). DEC-AXO-901594
        // Slice 1 : caller can override env-var resolution for per-test DBs.
        let pg_database_url = resolve_pg_database_url_with_override(database_url_override)
            .with_context(|| {
                "PostgreSQL is the only backend — set AXON_LIVE_DATABASE_URL, \
                 AXON_DEV_DATABASE_URL, or DATABASE_URL"
            })?;
        let writer_c_path = CString::new(pg_database_url)?;

        unsafe {
            let writer_ptr = init_fn(writer_c_path.as_ptr(), false);
            if writer_ptr.is_null() {
                return Err(anyhow!("Failed to init PostgreSQL writer"));
            }

            let pool = Arc::new(LatticePool {
                _lib: lib.clone(),
                symbols,
                writer_ctx: Mutex::new(writer_ptr),
            });
            let store = Self {
                pool: pool.clone(),
                soll_attached: !matches!(soll_access_mode, SollAccessMode::Detached),
                soll_read_only_mode: matches!(
                    soll_access_mode,
                    SollAccessMode::ReadOnlyOrEmptySchema
                ),
            };

            // MIL-AXO-015 P3 slice 3c: bootstrap the PG global schema
            // (extensions + soll layer) via the canonical DDL generator.
            // Per-project IST schemas are deferred to axon_init_project
            // (P5). PG dialect lives in `crate::postgres::ddl`.
            store.bootstrap_global_pg_schema()?;
            info!(
                "GraphStore startup: PostgreSQL global schema bootstrapped (CPT-AXO-039 + CPT-AXO-040 + CPT-AXO-041)."
            );

            Ok(store)
        }
    }

    /// REQ-AXO-901870 — the read path is the single PG writer connection
    /// pool (MVCC-consistent per statement). With the DuckDB-era reader
    /// replica retired there is no reader/writer epoch lag to track, so the
    /// IST snapshot read path is invariantly fresh. The orthogonal
    /// indexer-vs-source freshness signal (modified_files_since, CPT-AXO-029)
    /// is owned by the indexer_feed contract, not this read-path contract.
    pub(crate) fn ist_snapshot_freshness_contract(&self) -> RuntimeFreshnessContract {
        let stale_after_ms = std::env::var("AXON_IST_SNAPSHOT_STALE_AFTER_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(30_000)
            .max(1);
        RuntimeFreshnessContract::fresh(stale_after_ms)
    }

    /// `axon-plugin-postgres` cdylib discovery. Searches the standard
    /// cargo target directories under the workspace root + cwd.
    fn find_plugin_path() -> Result<String> {
        const CRATE_DIR: &str = "src/axon-plugin-postgres";
        const SO: &str = "libaxon_plugin_postgres.so";

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| anyhow!("Unable to resolve repository root from CARGO_MANIFEST_DIR"))?
            .to_path_buf();

        let mut candidates = vec![
            repo_root.join(format!("{CRATE_DIR}/target/release/{SO}")),
            repo_root.join(format!("{CRATE_DIR}/target/debug/{SO}")),
        ];

        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(format!("{CRATE_DIR}/target/release/{SO}")));
            candidates.push(cwd.join(format!("{CRATE_DIR}/target/debug/{SO}")));
        }

        for path in candidates {
            if path.exists() {
                return Ok(path.to_string_lossy().to_string());
            }
        }
        Err(anyhow!("Plugin not found (expected {})", SO))
    }

    /// MIL-AXO-015 P3 slice 3c: PostgreSQL global schema bootstrap.
    /// Idempotent. Executes the canonical DDL produced by
    /// `crate::postgres::ddl::generate_global_schema` (extensions +
    /// public.ProjectCodeRegistry + soll layer + cross-project
    /// indexes). Per-project IST schemas are created lazily by
    /// `axon_init_project` (P5).
    ///
    /// `CREATE EXTENSION` statements are run inside a graceful-degrade
    /// loop: if an extension is unavailable on the host PostgreSQL
    /// install (the image lacks AGE or pgvector), the bootstrap logs a
    /// warning and continues so the SOLL layer still comes up. Per
    /// DEC-AXO-075, production deployments MUST ship both extensions —
    /// the warning is the operator's signal to fix the install.
    ///
    /// Slice 5b: when `AXON_SOLL_SEED_PATH` points at a JSON seed and
    /// `soll.Node` is empty, load the snapshot via
    /// `crate::postgres::seed::load_seed_if_needed` so fresh
    /// deployments come up with canonical SOLL nodes preloaded.
    fn bootstrap_global_pg_schema(&self) -> Result<()> {
        // REQ-AXO-91562 — serialize bootstrap across parallel callers
        // (cargo test threads, concurrent embedded instances, etc.).
        // The PG catalog operations triggered by `CREATE OR REPLACE
        // FUNCTION`, `CREATE EXTENSION`, `CREATE TABLE IF NOT EXISTS`
        // are individually idempotent but DEAD-LOCK or fail on the
        // "already exists" path when two threads race on the same
        // shared catalog rows. A process-wide Mutex held for the
        // ~50 statements is much cheaper than the PG advisory-lock
        // alternative (one round-trip vs N) and is the canonical
        // pattern (mirrors `embedder_env_lock`).
        static BOOTSTRAP_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
            std::sync::OnceLock::new();
        let _guard = BOOTSTRAP_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for stmt in crate::postgres::ddl::generate_global_schema() {
            let trimmed = stmt.trim_start();
            let is_optional_extension = trimmed
                .to_uppercase()
                .starts_with("CREATE EXTENSION IF NOT EXISTS");
            match self.execute(&stmt) {
                Ok(()) => {}
                Err(err) if is_optional_extension => {
                    warn!(
                        statement = stmt.chars().take(80).collect::<String>().as_str(),
                        error = %err,
                        "PostgreSQL extension unavailable on this host; continuing without it. \
                         Install the extension to unlock dependent features (DEC-AXO-075)."
                    );
                }
                Err(err) => {
                    // REQ-AXO-901868 (lens #8 observabilité) : embarquer le
                    // message PG réel dans le contexte — pas seulement le
                    // statement tronqué à 80 chars, qui est souvent un
                    // commentaire `--` (split_top_level_statements garde les
                    // commentaires de tête attachés). En session 69 l'erreur
                    // réelle « gin_trgm_ops does not exist » était masquée
                    // derrière un commentaire affiché comme « statement »,
                    // forçant un diagnostic manuel par repro psql.
                    let pg_error = err.to_string();
                    let stmt_head: String = stmt
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty() && !l.starts_with("--"))
                        .unwrap_or_else(|| stmt.trim())
                        .chars()
                        .take(120)
                        .collect();
                    return Err(err).context(format!(
                        "PostgreSQL global schema bootstrap failed — PG error: \
                         {pg_error} (statement: {stmt_head})"
                    ));
                }
            }
        }

        if let Ok(seed_path) = std::env::var("AXON_SOLL_SEED_PATH") {
            if !seed_path.trim().is_empty() {
                let path = std::path::Path::new(seed_path.trim());
                match crate::postgres::seed::load_seed_if_needed(self, path) {
                    Ok(0) => {
                        info!(
                            seed_path = seed_path.as_str(),
                            "SOLL seed loader: nothing to load (file missing or SOLL non-empty)."
                        );
                    }
                    Ok(n) => {
                        info!(
                            seed_path = seed_path.as_str(),
                            inserted = n,
                            "SOLL seed loaded into fresh PostgreSQL deployment."
                        );
                    }
                    Err(err) => {
                        warn!(
                            seed_path = seed_path.as_str(),
                            error = %err,
                            "SOLL seed loader failed; brain is starting with whatever \
                             SOLL state currently exists. Re-run after fixing the seed file."
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn ensure_additive_soll_schema(&self) -> Result<()> {
        if !self.soll_attached || self.soll_read_only_mode {
            return Ok(());
        }

        self.execute("CREATE TABLE IF NOT EXISTS soll.Registry (project_code VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL', id VARCHAR DEFAULT 'AXON_GLOBAL', last_vis BIGINT DEFAULT 0, last_pil BIGINT DEFAULT 0, last_req BIGINT DEFAULT 0, last_cpt BIGINT DEFAULT 0, last_dec BIGINT DEFAULT 0, last_mil BIGINT DEFAULT 0, last_val BIGINT DEFAULT 0, last_stk BIGINT DEFAULT 0, last_gui BIGINT DEFAULT 0, last_ski BIGINT DEFAULT 0, last_prt BIGINT DEFAULT 0, last_prv BIGINT DEFAULT 0, last_rev BIGINT DEFAULT 0)")?;
        let _ = self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_gui BIGINT DEFAULT 0",
        );
        // REQ-AXO-91578: SKI (Skill) entity counter — additive migration.
        let _ = self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_ski BIGINT DEFAULT 0",
        );
        // REQ-AXO-91579: PRT (PromptTemplate) entity counter — additive migration.
        let _ = self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_prt BIGINT DEFAULT 0",
        );
        self.execute("CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (project_code VARCHAR PRIMARY KEY, project_name VARCHAR, project_path VARCHAR)")?;
        self.execute(
            "ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS project_name VARCHAR",
        )?;
        self.execute(
            "ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS project_path VARCHAR",
        )?;
        // REQ-AXO-143 — per-project session pointer (file|url|soll_node|none).
        // Stored as serialized JSON object: {kind, value, label?}.
        // NULL when the project does not declare a session-pointer convention.
        self.execute(
            "ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS session_pointer_json VARCHAR",
        )?;
        self.normalize_project_code_registry_schema()?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS soll_project_code_registry_code_idx ON soll.ProjectCodeRegistry(project_code)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Node (id VARCHAR PRIMARY KEY, type VARCHAR, project_code VARCHAR, title VARCHAR, description VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Edge (source_id VARCHAR, target_id VARCHAR, relation_type VARCHAR, metadata VARCHAR, PRIMARY KEY (source_id, target_id, relation_type))")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.McpJob (job_id VARCHAR PRIMARY KEY, tool_name VARCHAR, status VARCHAR, submitted_at BIGINT, started_at BIGINT, finished_at BIGINT, request_json VARCHAR, reserved_ids_json VARCHAR, result_json VARCHAR, error_text VARCHAR)")?;

        // Performance Indexes
        self.execute("CREATE INDEX IF NOT EXISTS soll_node_type_idx ON soll.Node(type)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_node_project_code_idx ON soll.Node(project_code)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_source_idx ON soll.Edge(source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_target_idx ON soll.Edge(target_id)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_edge_relation_idx ON soll.Edge(relation_type)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_mcp_job_status_idx ON soll.McpJob(status)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_mcp_job_submitted_idx ON soll.McpJob(submitted_at)",
        )?;

        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_pil BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_vis BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_stk BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_prv BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_rev BIGINT DEFAULT 0",
        )?;

        self.execute("CREATE TABLE IF NOT EXISTS soll.Revision (revision_id VARCHAR PRIMARY KEY, author VARCHAR, source VARCHAR, summary VARCHAR, status VARCHAR, created_at BIGINT, committed_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.RevisionChange (revision_id VARCHAR, entity_type VARCHAR, entity_id VARCHAR, action VARCHAR, before_json VARCHAR, after_json VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.RevisionPreview (preview_id VARCHAR PRIMARY KEY, author VARCHAR, project_code VARCHAR, payload VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Traceability (id VARCHAR PRIMARY KEY, soll_entity_type VARCHAR, soll_entity_id VARCHAR, artifact_type VARCHAR, artifact_ref VARCHAR, confidence DOUBLE, metadata VARCHAR, created_at BIGINT)")?;

        // REQ-AXO-066 Phase 1 (DEC-AXO-064 Option A): denormalize project_code on
        // the remaining SOLL tables so per-tenant filtering does not require
        // joining soll.Node every time.
        self.execute("ALTER TABLE soll.Edge ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE soll.McpJob ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE soll.Revision ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute(
            "ALTER TABLE soll.RevisionChange ADD COLUMN IF NOT EXISTS project_code VARCHAR",
        )?;

        // Backfill is idempotent: edges inherit from the source Node when known,
        // everything else falls back to 'AXO' since pre-Phase-1 rows predate
        // multi-tenant scoping (single-project history).
        self.execute(
            "UPDATE soll.Edge SET project_code = COALESCE(
                NULLIF(soll.Edge.project_code, ''),
                (SELECT n.project_code FROM soll.Node n WHERE n.id = soll.Edge.source_id),
                'AXO'
            ) WHERE soll.Edge.project_code IS NULL OR soll.Edge.project_code = ''",
        )?;
        // DuckDB upstream issue #15836: UPDATE on a primary-keyed row
        // internally does DELETE+INSERT. For soll.McpJob's legacy rows that
        // were committed under different transaction shapes, this corrupts
        // the PK index — once corrupted, the UPDATE crashes the brain on
        // every boot AND a plain DELETE fails too ("Failed to delete all
        // rows from index. Only deleted 0 out of 4 rows."). Skip the
        // backfill when legacy NULL rows exist; emit a warning so we know
        // the table still needs migration. Proper fix: CTAS rebuild of
        // soll.McpJob OR bumping the bundled DuckDB to a version that ships
        // #15836's patch. Boot stays unblocked either way.
        let mcp_job_needs_backfill: i64 = self.query_count(
            "SELECT count(*) FROM soll.McpJob WHERE project_code IS NULL OR project_code = ''",
        )?;
        if mcp_job_needs_backfill > 0 {
            tracing::warn!(
                count = mcp_job_needs_backfill,
                reason = "duckdb_15836_workaround",
                "soll_mcpjob_backfill_skipped: legacy rows with NULL project_code retained to avoid PK-index corruption on UPDATE; CTAS rebuild required for proper migration"
            );
        }
        self.execute("UPDATE soll.Revision SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = ''")?;
        self.execute("UPDATE soll.RevisionChange SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = ''")?;

        // Composite (project_code, key) indexes for hot SOLL multi-tenant lookups.
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_node_project_id_idx ON soll.Node(project_code, id)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_project_source_idx ON soll.Edge(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_project_target_idx ON soll.Edge(project_code, target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_mcp_job_project_status_idx ON soll.McpJob(project_code, status)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_revision_project_idx ON soll.Revision(project_code, created_at)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_revision_change_project_idx ON soll.RevisionChange(project_code, revision_id)")?;

        self.normalize_project_code_registry()?;
        self.normalize_soll_registry()?;
        self.normalize_revision_preview_schema()?;
        // DEC-AXO-082 seed half (REQ-AXO-91577) — replaces the legacy
        // `seed_project_code_registry` + `seed_global_guidelines` Rust
        // paths. Canonical PRO seed (registry row, Pillars, Concepts,
        // Decisions, Guidelines, edges) lives in `db/seed/01_global_soll.sql`
        // and is applied by `scripts/lib/ensure-runtime.sh` in production ;
        // the Rust path here gives the test harness consistent data via
        // the same SQL file. Idempotent — INSERT ... ON CONFLICT DO NOTHING.
        self.apply_canonical_seed_files()?;
        Ok(())
    }

    /// DEC-AXO-082 seed half (REQ-AXO-91577) — apply canonical SQL seed
    /// files from `db/seed/[0-9][0-9]_*.sql` in lexical order.
    ///
    /// Path resolution :
    ///   1. `AXON_SEED_DIR` env var (operator override)
    ///   2. Repo-root via `CARGO_MANIFEST_DIR/../../db/seed` (works in tests
    ///      and in cargo-run from source)
    ///   3. If neither resolves : log info and skip — the production binary
    ///      relies on `scripts/lib/ensure-runtime.sh apply_canonical_seed`
    ///      to psql-apply the same files before brain start.
    fn apply_canonical_seed_files(&self) -> Result<()> {
        let seed_dir = if let Ok(env_path) = std::env::var("AXON_SEED_DIR") {
            PathBuf::from(env_path)
        } else {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            manifest
                .parent()
                .and_then(|p| p.parent())
                .map(|repo| repo.join("db").join("seed"))
                .unwrap_or_default()
        };
        if !seed_dir.is_dir() {
            info!(
                seed_dir = %seed_dir.display(),
                "apply_canonical_seed_files: no db/seed directory found; relying on ensure-runtime.sh"
            );
            return Ok(());
        }
        let applied = crate::postgres::seed::apply_canonical_seed_dir(self, &seed_dir)
            .with_context(|| format!("apply canonical seed from {}", seed_dir.display()))?;
        if applied > 0 {
            info!(
                applied = applied,
                seed_dir = %seed_dir.display(),
                "DEC-AXO-082 seed applied"
            );
        }
        Ok(())
    }

    /// REQ-AXO-901876 — PG replacement for the DuckDB/SQLite
    /// `pragma_table_info` (which does not exist in PostgreSQL —
    /// SQLSTATE 42883). Returns the column names of `target` from
    /// `information_schema.columns`. `target` may be `schema.table` or a
    /// bare `table`; PostgreSQL folds unquoted identifiers to lowercase,
    /// so both the lookup and the returned names are lowercase (callers
    /// compare case-insensitively).
    fn table_column_names(&self, target: &str) -> Result<Vec<String>> {
        let where_clause = match target.split_once('.') {
            Some((schema, table)) => format!(
                "table_schema = '{}' AND table_name = '{}'",
                schema.to_lowercase().replace('\'', "''"),
                table.to_lowercase().replace('\'', "''"),
            ),
            None => format!("table_name = '{}'", target.to_lowercase().replace('\'', "''")),
        };
        let raw = self.query_json(&format!(
            "SELECT column_name FROM information_schema.columns WHERE {where_clause}"
        ))?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|mut row| (!row.is_empty()).then(|| row.remove(0)))
            .collect())
    }

    fn normalize_soll_registry(&self) -> Result<()> {
        let columns = self.table_column_names("soll.Registry")?;
        let has_project_code = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_code"));
        let has_project_slug = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_slug"));

        if !has_project_code {
            return Err(anyhow!(
                "Legacy soll.Registry schema detected: missing canonical project_code column"
            ));
        }
        if has_project_slug {
            return Err(anyhow!(
                "Legacy soll.Registry schema detected: forbidden project_slug column still present"
            ));
        }

        let raw_rows = self.query_json(
            "SELECT
                COALESCE(NULLIF(TRIM(project_code), ''), ''),
                COALESCE(id, 'AXON_GLOBAL')
             FROM soll.Registry",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw_rows).unwrap_or_default();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let project_code = row[0].trim();
            if project_code.is_empty() || !crate::project_meta::is_valid_project_code(project_code)
            {
                return Err(anyhow!(
                    "Invalid project_code in soll.Registry: {}",
                    project_code
                ));
            }
            let resolved = self.query_count(&format!(
                "SELECT count(*) FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
                project_code.replace('\'', "''")
            ))?;
            if resolved == 0 {
                return Err(anyhow!(
                    "Unknown project_code in soll.Registry: {}",
                    project_code
                ));
            }
        }
        Ok(())
    }

    fn normalize_revision_preview_schema(&self) -> Result<()> {
        let columns = self.table_column_names("soll.RevisionPreview")?;
        let has_project_code = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_code"));
        let has_project_slug = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_slug"));

        if !has_project_code {
            return Err(anyhow!(
                "Legacy soll.RevisionPreview schema detected: missing canonical project_code column"
            ));
        }
        if has_project_slug {
            return Err(anyhow!(
                "Legacy soll.RevisionPreview schema detected: forbidden project_slug column still present"
            ));
        }

        let raw_rows = self.query_json(
            "SELECT
                preview_id,
                COALESCE(author, ''),
                COALESCE(NULLIF(TRIM(project_code), ''), ''),
                COALESCE(payload, ''),
                COALESCE(created_at, 0)
             FROM soll.RevisionPreview
             ORDER BY created_at ASC, preview_id ASC",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw_rows).unwrap_or_default();

        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let preview_id = row[0].trim();
            if preview_id.is_empty() {
                continue;
            }
            let project_code = row[2].trim();
            if project_code.is_empty() || !crate::project_meta::is_valid_project_code(project_code)
            {
                return Err(anyhow!(
                    "Invalid project_code in soll.RevisionPreview: {}",
                    project_code
                ));
            }
            let resolved = self.query_count(&format!(
                "SELECT count(*) FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
                project_code.replace('\'', "''")
            ))?;
            if resolved == 0 {
                return Err(anyhow!(
                    "Unknown project_code in soll.RevisionPreview: {}",
                    project_code
                ));
            }

            if let Some((_, preview_code, _)) = parse_prefixed_entity_id(preview_id) {
                if preview_code != project_code {
                    return Err(anyhow!(
                        "RevisionPreview project_code mismatch: preview_id={} project_code={}",
                        preview_id,
                        project_code
                    ));
                }
            }
        }
        Ok(())
    }

    fn normalize_project_code_registry_schema(&self) -> Result<()> {
        let columns = self.table_column_names("soll.ProjectCodeRegistry")?;
        let has_project_code = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_code"));
        let has_project_name = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_name"));
        let has_project_path = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_path"));
        let has_legacy_slug = columns
            .iter()
            .any(|value| value.eq_ignore_ascii_case("project_slug"));

        if !has_project_code || !has_project_name || !has_project_path {
            return Err(anyhow!(
                "Legacy soll.ProjectCodeRegistry schema detected: canonical columns are incomplete"
            ));
        }
        if has_legacy_slug {
            return Err(anyhow!(
                "Legacy soll.ProjectCodeRegistry schema detected: forbidden project_slug column still present"
            ));
        }
        Ok(())
    }

    fn normalize_project_code_registry(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT COALESCE(project_code,''), COALESCE(project_name,''), COALESCE(project_path,'')
             FROM soll.ProjectCodeRegistry",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let existing_project_code = row[0].trim().to_ascii_uppercase();
            let existing_project_name = row[1].trim().to_string();
            let project_path = row[2].trim().to_string();
            if existing_project_code.is_empty()
                || !crate::project_meta::is_valid_project_code(&existing_project_code)
            {
                continue;
            }

            let normalized_name = std::path::Path::new(&project_path)
                .file_name()
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    (!existing_project_name.is_empty()).then_some(existing_project_name.clone())
                })
                .unwrap_or_else(|| existing_project_code.clone());

            if existing_project_name != normalized_name {
                self.execute_param(
                    "UPDATE soll.ProjectCodeRegistry SET project_name = ? WHERE project_code = ?",
                    &serde_json::json!([normalized_name, existing_project_code]),
                )?;
            }

            if !project_path.is_empty() {
                self.execute_param(
                    "UPDATE soll.ProjectCodeRegistry SET project_path = ? WHERE project_code = ?",
                    &serde_json::json!([project_path, existing_project_code]),
                )?;
            }
        }
        Ok(())
    }

    /// DEC-AXO-082 seed half (REQ-AXO-91577) — retired. The global
    /// GUI-PRO-* guidelines (and the rest of the PRO methodology surface :
    /// Pillars, Concepts, Decisions, registry rows, edges) now live in
    /// `db/seed/01_global_soll.sql`, applied via `apply_canonical_seed_files`
    /// (or `scripts/lib/ensure-runtime.sh` in production). Signature
    /// retained for binary-API stability per DEC-AXO-082 consequence ;
    /// body removed. The hardcoded array of ~17 guidelines that lived here
    /// drifted over time vs. live SOLL (which carried 33 GUI-PRO entries
    /// post-soll_manager mutations) — the SQL seed captures the canonical
    /// state and removes the drift-by-Rust-fork failure mode.
    fn seed_global_guidelines(&self) -> Result<()> {
        info!("seed_global_guidelines: retired (DEC-AXO-082 — see db/seed/01_global_soll.sql)");
        Ok(())
    }

    /// DEC-AXO-082 seed half (REQ-AXO-91577) — retired. The PRO sentinel
    /// row now lives in `db/seed/01_global_soll.sql`, applied via
    /// `apply_canonical_seed_files` (or `scripts/lib/ensure-runtime.sh`
    /// in production). Signature retained for binary-API stability per
    /// DEC-AXO-082 consequence.
    fn seed_project_code_registry(&self) -> Result<()> {
        info!("seed_project_code_registry: retired (DEC-AXO-082 — see db/seed/01_global_soll.sql)");
        Ok(())
    }

    pub(crate) fn sync_project_registry_entry(
        &self,
        project_code: &str,
        project_name: Option<&str>,
        project_path: Option<&str>,
    ) -> Result<()> {
        let normalized_code = project_code.trim().to_ascii_uppercase();
        let normalized_name = project_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                project_path
                    .and_then(|path| std::path::Path::new(path).file_name())
                    .map(|value| value.to_string_lossy().trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| normalized_code.clone());
        if normalized_code.is_empty()
            || !crate::project_meta::is_valid_project_code(&normalized_code)
        {
            return Ok(());
        }

        let normalized_path = project_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        self.execute_param(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) VALUES (?, ?, ?) ON CONFLICT (project_code) DO UPDATE SET project_name = EXCLUDED.project_name, project_path = EXCLUDED.project_path",
            &serde_json::json!([normalized_code, normalized_name, normalized_path]),
        )?;

        Ok(())
    }

    /// REQ-AXO-143 — persist a project's session pointer (file|url|soll_node|none).
    /// `pointer` is the canonical JSON object `{kind, value, label?}` or `None`
    /// to clear the field. Idempotent.
    pub(crate) fn write_session_pointer(
        &self,
        project_code: &str,
        pointer: Option<&serde_json::Value>,
    ) -> Result<()> {
        let normalized_code = project_code.trim().to_ascii_uppercase();
        if normalized_code.is_empty()
            || !crate::project_meta::is_valid_project_code(&normalized_code)
        {
            return Ok(());
        }
        let serialized = pointer
            .map(serde_json::Value::to_string)
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null);
        self.execute_param(
            "UPDATE soll.ProjectCodeRegistry SET session_pointer_json = ? WHERE project_code = ?",
            &serde_json::json!([serialized, normalized_code]),
        )?;
        Ok(())
    }

    /// REQ-AXO-143 — read a project's session pointer; returns `None` when
    /// the column is NULL or carries an unparseable string.
    pub(crate) fn read_session_pointer(
        &self,
        project_code: &str,
    ) -> Result<Option<serde_json::Value>> {
        let normalized_code = project_code.trim().to_ascii_uppercase();
        if normalized_code.is_empty() {
            return Ok(None);
        }
        let raw = self.query_json_param(
            "SELECT COALESCE(session_pointer_json, '') FROM soll.ProjectCodeRegistry WHERE project_code = ? LIMIT 1",
            &serde_json::json!([normalized_code]),
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let payload = row.first().map(String::as_str).unwrap_or("").trim();
        if payload.is_empty() {
            return Ok(None);
        }
        Ok(serde_json::from_str::<serde_json::Value>(payload).ok())
    }

    fn migrate_canonical_soll_ids(&self) -> Result<()> {
        self.migrate_prefixed_id_table("soll.Vision")?;
        self.migrate_prefixed_id_table("soll.Pillar")?;
        self.migrate_prefixed_id_table("soll.Requirement")?;
        self.migrate_prefixed_id_table("soll.Decision")?;
        self.migrate_prefixed_id_table("soll.Milestone")?;
        self.migrate_prefixed_id_table("soll.Validation")?;
        self.migrate_concepts_to_server_ids()?;
        self.migrate_stakeholders_to_server_ids()?;
        self.migrate_revision_preview_ids()?;
        self.migrate_revision_ids()?;
        Ok(())
    }

    fn migrate_revision_preview_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT preview_id, COALESCE(project_code,''), COALESCE(created_at, 0)
             FROM soll.RevisionPreview
             ORDER BY created_at ASC, preview_id ASC",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut next_by_code: HashMap<String, u64> = HashMap::new();

        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let old_id = row[0].trim().to_string();
            let project_code = row[1].trim().to_string();
            if old_id.is_empty() || project_code.is_empty() {
                continue;
            }
            let (_, project_code) =
                self.resolve_or_seed_existing_project_identity(&project_code)?;
            let next = next_by_code.get(&project_code).copied().unwrap_or(0) + 1;
            next_by_code.insert(project_code.clone(), next);
            let new_id = format!("PRV-{}-{:03}", project_code, next);

            if old_id == new_id {
                continue;
            }

            if self.table_has_named_id("soll.RevisionPreview", "preview_id", &new_id)? {
                self.delete_row_by_named_id("soll.RevisionPreview", "preview_id", &old_id)?;
            } else {
                self.execute_param(
                    "UPDATE soll.RevisionPreview SET preview_id = ? WHERE preview_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
            }
        }
        Ok(())
    }

    fn migrate_revision_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT revision_id, COALESCE(created_at, 0)
             FROM soll.Revision
             ORDER BY created_at ASC, revision_id ASC",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut next_by_code: HashMap<String, u64> = HashMap::new();

        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let old_id = row[0].trim().to_string();
            let Some((_, project_part, _)) = parse_prefixed_entity_id(&old_id) else {
                continue;
            };
            let (_, project_code) = self.resolve_or_seed_existing_project_identity(project_part)?;
            let next = next_by_code.get(&project_code).copied().unwrap_or(0) + 1;
            next_by_code.insert(project_code.clone(), next);
            let new_id = format!("REV-{}-{:03}", project_code, next);

            if old_id == new_id {
                continue;
            }

            if self.table_has_named_id("soll.Revision", "revision_id", &new_id)? {
                self.execute_param(
                    "UPDATE soll.RevisionChange SET revision_id = ? WHERE revision_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
                self.delete_row_by_named_id("soll.Revision", "revision_id", &old_id)?;
            } else {
                self.execute_param(
                    "UPDATE soll.Revision SET revision_id = ? WHERE revision_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
                self.execute_param(
                    "UPDATE soll.RevisionChange SET revision_id = ? WHERE revision_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
            }
        }
        Ok(())
    }

    fn migrate_prefixed_id_table(&self, table: &str) -> Result<()> {
        let raw = self.query_json(&format!("SELECT id FROM {} ORDER BY id", table))?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            let Some(old_id) = row.first().cloned() else {
                continue;
            };
            let Some((prefix, project_part, number)) = parse_prefixed_entity_id(&old_id) else {
                continue;
            };
            let (_, project_code) = self.resolve_or_seed_existing_project_identity(project_part)?;
            let new_id = format!("{}-{}-{:03}", prefix, project_code, number);
            if new_id != old_id {
                if self.table_has_id(table, &new_id)? {
                    self.replace_soll_id_references(&old_id, &new_id)?;
                    self.delete_row_by_id(table, &old_id)?;
                } else {
                    self.execute_param(
                        &format!("UPDATE {} SET id = ? WHERE id = ?", table),
                        &serde_json::json!([new_id, old_id]),
                    )?;
                    self.replace_soll_id_references(&old_id, &new_id)?;
                }
            }
            if table == "soll.Vision" {
                self.execute_param(
                    "UPDATE soll.Vision SET project_code = ? WHERE id = ?",
                    &serde_json::json!([project_code, new_id]),
                )?;
            }
        }
        Ok(())
    }

    fn migrate_concepts_to_server_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT COALESCE(id,''), COALESCE(project_code,''), title
             FROM soll.Node WHERE type='Concept'
             ORDER BY title",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let existing_id = row[0].clone();
            let existing_project_code = row[1].clone();
            let stored_name = row[2].clone();

            let source_id = if !existing_id.trim().is_empty() {
                existing_id.clone()
            } else if let Some((parsed_id, parsed_name)) = split_prefixed_display_name(&stored_name)
            {
                let _ = parsed_name;
                parsed_id
            } else {
                continue;
            };

            let Some((_, project_part, number)) = parse_prefixed_entity_id(&source_id) else {
                continue;
            };
            let project_code = if !existing_project_code.trim().is_empty() {
                existing_project_code.clone()
            } else {
                self.resolve_or_seed_existing_project_identity(project_part)?
                    .1
            };
            let new_id = format!("CPT-{}-{:03}", project_code, number);

            if new_id == existing_id && existing_project_code == project_code {
                continue;
            }

            if new_id != source_id && self.table_has_id("soll.Concept", &new_id)? {
                self.replace_soll_id_references(&source_id, &new_id)?;
                self.execute_param(
                    "DELETE FROM soll.Node WHERE type='Concept' AND COALESCE(id,'') = ? AND title = ?",
                    &serde_json::json!([existing_id, stored_name]),
                )?;
            } else if new_id == existing_id {
                self.execute_param(
                    "UPDATE soll.Concept
                     SET project_code = ?
                     WHERE id = ?",
                    &serde_json::json!([project_code, existing_id]),
                )?;
            } else {
                self.execute_param(
                    "UPDATE soll.Concept
                     SET id = ?, project_code = ?
                     WHERE COALESCE(id,'') = ? AND name = ?",
                    &serde_json::json!([new_id, project_code, existing_id, stored_name]),
                )?;

                if new_id != source_id {
                    self.replace_soll_id_references(&source_id, &new_id)?;
                }
            }
        }
        Ok(())
    }

    fn migrate_stakeholders_to_server_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT COALESCE(id,''), COALESCE(project_code,''), title
             FROM soll.Node WHERE type='Stakeholder'
             ORDER BY title",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut next_by_code: HashMap<String, u64> = HashMap::new();

        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let existing_id = row[0].clone();
            let existing_project_code = row[1].clone();
            let name = row[2].clone();

            let (project_code, source_id, new_id) = if let Some((prefix, project_part, number)) =
                parse_prefixed_entity_id(&existing_id)
            {
                let code = if !existing_project_code.trim().is_empty() {
                    existing_project_code.clone()
                } else {
                    self.resolve_or_seed_existing_project_identity(project_part)?
                        .1
                };
                (
                    code.clone(),
                    existing_id.clone(),
                    format!("{}-{}-{:03}", prefix, code, number),
                )
            } else {
                let initial_code = if existing_project_code.trim().is_empty() {
                    "AXO".to_string()
                } else {
                    existing_project_code.clone()
                };
                let (_, code) = self.resolve_or_seed_existing_project_identity(&initial_code)?;
                let next = match next_by_code.get(&code).copied() {
                    Some(current) => current + 1,
                    None => self.max_numeric_suffix_for_prefix(&format!("STK-{}-", code))? + 1,
                };
                next_by_code.insert(code.clone(), next);
                (
                    code.clone(),
                    if existing_id.trim().is_empty() {
                        name.clone()
                    } else {
                        existing_id.clone()
                    },
                    format!("STK-{}-{:03}", code, next),
                )
            };

            if new_id == existing_id && existing_project_code == project_code {
                continue;
            }

            if new_id != source_id && self.table_has_id("soll.Stakeholder", &new_id)? {
                self.replace_soll_id_references(&source_id, &new_id)?;
                self.execute_param(
                    "DELETE FROM soll.Node WHERE type='Stakeholder' AND COALESCE(id,'') = ? AND title = ?",
                    &serde_json::json!([existing_id, name]),
                )?;
            } else if new_id == existing_id {
                self.execute_param(
                    "UPDATE soll.Stakeholder
                     SET project_code = ?
                     WHERE id = ?",
                    &serde_json::json!([project_code, existing_id]),
                )?;
            } else {
                self.execute_param(
                    "UPDATE soll.Stakeholder
                     SET id = ?, project_code = ?
                     WHERE COALESCE(id,'') = ? AND name = ?",
                    &serde_json::json!([new_id, project_code, existing_id, name]),
                )?;

                if new_id != source_id {
                    self.replace_soll_id_references(&source_id, &new_id)?;
                }
            }
        }
        Ok(())
    }

    fn table_has_id(&self, table: &str, id: &str) -> Result<bool> {
        Ok(self.query_count(&format!(
            "SELECT count(*) FROM {} WHERE id = '{}'",
            table,
            id.replace('\'', "''")
        ))? > 0)
    }

    fn table_has_named_id(&self, table: &str, column: &str, id: &str) -> Result<bool> {
        Ok(self.query_count(&format!(
            "SELECT count(*) FROM {} WHERE {} = '{}'",
            table,
            column,
            id.replace('\'', "''")
        ))? > 0)
    }

    fn delete_row_by_id(&self, table: &str, id: &str) -> Result<()> {
        self.execute_param(
            &format!("DELETE FROM {} WHERE id = ?", table),
            &serde_json::json!([id]),
        )?;
        Ok(())
    }

    fn delete_row_by_named_id(&self, table: &str, column: &str, id: &str) -> Result<()> {
        self.execute_param(
            &format!("DELETE FROM {} WHERE {} = ?", table, column),
            &serde_json::json!([id]),
        )?;
        Ok(())
    }

    fn max_numeric_suffix_for_prefix(&self, prefix: &str) -> Result<u64> {
        let mut max_seen = 0u64;
        for table in [
            "soll.Vision",
            "soll.Pillar",
            "soll.Requirement",
            "soll.Decision",
            "soll.Milestone",
            "soll.Validation",
            "soll.Concept",
            "soll.Stakeholder",
        ] {
            let id_col = "id";
            let raw = self.query_json(&format!(
                "SELECT {} FROM {} WHERE {} LIKE '{}%'",
                id_col,
                table,
                id_col,
                prefix.replace('\'', "''")
            ))?;
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if let Some(id) = row.first() {
                    if let Some((_, _, number)) = parse_prefixed_entity_id(id) {
                        max_seen = max_seen.max(number);
                    }
                }
            }
        }
        Ok(max_seen)
    }

    fn resolve_or_seed_existing_project_identity(
        &self,
        project_code: &str,
    ) -> Result<(String, String)> {
        let key = project_code.trim();
        if key.is_empty() {
            return Err(anyhow!("Empty project identifier"));
        }

        let by_code = self.query_json(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            key.replace('\'', "''")
        ))?;
        let code_rows: Vec<Vec<String>> = serde_json::from_str(&by_code).unwrap_or_default();
        if let Some(row) = code_rows.first() {
            if let Some(code) = row.first() {
                return Ok((code.clone(), code.clone()));
            }
        }

        Err(anyhow!("Missing project code registry entry for {}", key))
    }

    fn replace_soll_id_references(&self, old_id: &str, new_id: &str) -> Result<()> {
        if old_id == new_id {
            return Ok(());
        }
        for table in [
            "soll.EPITOMIZES",
            "soll.BELONGS_TO",
            "soll.EXPLAINS",
            "soll.SOLVES",
            "soll.TARGETS",
            "soll.VERIFIES",
            "soll.ORIGINATES",
            "soll.SUPERSEDES",
            "soll.CONTRIBUTES_TO",
            "soll.REFINES",
            "IMPACTS",
            "SUBSTANTIATES",
        ] {
            self.execute_param(
                &format!("UPDATE {} SET source_id = ? WHERE source_id = ?", table),
                &serde_json::json!([new_id, old_id]),
            )?;
            self.execute_param(
                &format!("UPDATE {} SET target_id = ? WHERE target_id = ?", table),
                &serde_json::json!([new_id, old_id]),
            )?;
        }

        self.execute_param(
            "UPDATE soll.Traceability SET soll_entity_id = ? WHERE soll_entity_id = ?",
            &serde_json::json!([new_id, old_id]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionChange SET entity_id = ? WHERE entity_id = ?",
            &serde_json::json!([new_id, old_id]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionChange SET before_json = REPLACE(before_json, ?, ?) WHERE before_json LIKE ?",
            &serde_json::json!([old_id, new_id, format!("%{}%", old_id)]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionChange SET after_json = REPLACE(after_json, ?, ?) WHERE after_json LIKE ?",
            &serde_json::json!([old_id, new_id, format!("%{}%", old_id)]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionPreview SET payload = REPLACE(payload, ?, ?) WHERE payload LIKE ?",
            &serde_json::json!([old_id, new_id, format!("%{}%", old_id)]),
        )?;
        Ok(())
    }

    // REQ-AXO-901653 slice-5a: `ensure_runtime_compatibility` +
    // `recover_interrupted_indexing` deleted ; both queried/updated
    // public.File status state machine columns (graph_ready /
    // vector_ready / file_stage / status). Pipeline-v2 (REQ-AXO-289)
    // makes the per-file recovery cursor obsolete — A/B stages are
    // idempotent and replay from ist.IndexedFile + ist.Chunk.

    fn load_runtime_metadata(&self) -> Result<std::collections::HashMap<String, String>> {
        let existing = self.query_json("SELECT key, value FROM RuntimeMetadata")?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&existing).unwrap_or_default();
        let mut current = std::collections::HashMap::new();
        for row in rows {
            if row.len() >= 2 {
                current.insert(row[0].clone(), row[1].clone());
            }
        }
        Ok(current)
    }

    fn write_runtime_metadata(&self, expected: &[(&str, &str)]) -> Result<()> {
        self.execute("DELETE FROM RuntimeMetadata")?;
        for (key, value) in expected {
            self.execute(&format!(
                "INSERT INTO RuntimeMetadata (key, value) VALUES ('{}', '{}')",
                key, value
            ))?;
        }
        Ok(())
    }

    // REQ-AXO-901653 slice-5a: `is_known_additive_schema_repair` +
    // `list_file_table_columns` deleted ; introspected the retired
    // public.File schema (status / file_stage / graph_ready /
    // vector_ready columns).

    fn list_project_code_registry_columns(&self) -> Result<std::collections::HashSet<String>> {
        for target in ["soll.ProjectCodeRegistry", "ProjectCodeRegistry"] {
            let columns: std::collections::HashSet<String> =
                self.table_column_names(target)?.into_iter().collect();
            if !columns.is_empty() {
                return Ok(columns);
            }
        }
        Ok(std::collections::HashSet::new())
    }

    fn list_soll_node_columns(&self) -> Result<std::collections::HashSet<String>> {
        for target in ["soll.Node", "Node"] {
            let columns: std::collections::HashSet<String> =
                self.table_column_names(target)?.into_iter().collect();
            if !columns.is_empty() {
                return Ok(columns);
            }
        }
        Ok(std::collections::HashSet::new())
    }

    // REQ-AXO-901653 slice-5a: `reset_ist_state`,
    // `soft_invalidate_derived_state`, `soft_invalidate_embedding_state`,
    // `rebuild_file_runtime_table` deleted ; all rebuilt the retired
    // public.File table + its file_project_code_idx / file_status_idx /
    // file_project_path_idx + reset graph_ready / vector_ready /
    // file_stage. Their only caller (`ensure_runtime_compatibility`) was
    // already deleted above.
}

#[cfg(test)]
mod graph_bootstrap_tests {
    use super::{startup_vector_backfill_limit, GraphStore, STARTUP_SEMANTIC_BACKFILL_FLOOR};
    use crate::tests::test_helpers::create_test_db;
    use tempfile::tempdir;

    #[test]
    fn test_normalize_project_code_registry_mirrors_code_and_derives_name_from_path() {
        let store = create_test_db().unwrap();
        store
            .execute_param(
                "UPDATE soll.ProjectCodeRegistry
                 SET project_code = ?, project_name = ?, project_path = ?
                 WHERE project_code = ?",
                &serde_json::json!([
                    "BKS",
                    "Legacy Human Name",
                    "/home/dstadel/projects/BookingSystem",
                    "BKS"
                ]),
            )
            .unwrap();

        store.normalize_project_code_registry().unwrap();

        let rows = store
            .query_json(
                "SELECT project_code, project_name, project_path
                 FROM soll.ProjectCodeRegistry
                 WHERE project_code = 'BKS'",
            )
            .unwrap();
        let parsed: Vec<Vec<String>> = serde_json::from_str(&rows).unwrap();
        let row = parsed.first().expect("registry row");
        assert_eq!(row[0], "BKS");
        assert_eq!(row[1], "BookingSystem");
        assert_eq!(row[2], "/home/dstadel/projects/BookingSystem");
    }

    #[test]
    fn test_normalize_soll_registry_accepts_canonical_schema() {
        let store = create_test_db().unwrap();

        store.normalize_soll_registry().unwrap();
    }

    #[test]
    fn test_normalize_soll_registry_rejects_legacy_slug_column() {
        let store = create_test_db().unwrap();
        store.execute("DROP TABLE soll.Registry").unwrap();
        store
            .execute(
                "CREATE TABLE soll.Registry (
                project_slug VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL',
                id VARCHAR DEFAULT 'AXON_GLOBAL',
                last_req BIGINT DEFAULT 0,
                last_cpt BIGINT DEFAULT 0,
                last_dec BIGINT DEFAULT 0,
                last_mil BIGINT DEFAULT 0,
                last_val BIGINT DEFAULT 0,
                last_pil BIGINT DEFAULT 0,
                last_vis BIGINT DEFAULT 0,
                last_stk BIGINT DEFAULT 0,
                last_prv BIGINT DEFAULT 0,
                last_rev BIGINT DEFAULT 0,
                last_gui BIGINT DEFAULT 0
            )",
            )
            .unwrap();

        let err = store.normalize_soll_registry().unwrap_err();
        assert!(err
            .to_string()
            .contains("Legacy soll.Registry schema detected"));
    }

    #[test]
    fn test_normalize_project_code_registry_schema_accepts_canonical_schema() {
        let store = create_test_db().unwrap();
        store.normalize_project_code_registry_schema().unwrap();
    }

    #[test]
    fn test_indexer_store_can_boot_while_brain_holds_soll_writer() {
        let temp = tempdir().unwrap();
        let db_root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&db_root).unwrap();
        let db_root_str = db_root.to_string_lossy().to_string();

        let brain = GraphStore::new_brain_reader_soll_writer(&db_root_str).unwrap();
        brain
            .execute(
                "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path)
                 VALUES ('AXO', 'Axon', '/home/dstadel/projects/axon')
                 ON CONFLICT (project_code) DO NOTHING",
            )
            .unwrap();

        let indexer = GraphStore::new_indexer_ist_writer_without_soll(&db_root_str).unwrap();
        assert!(!indexer.soll_attached);
        indexer
            .execute(
                "INSERT INTO ist.IndexedFile (path, content_hash, last_seen_ms)
                 VALUES ('/tmp/indexer.txt', 'hash-indexer', 1)",
            )
            .unwrap();
        // REQ-AXO-901870 — brain + indexer coexist on the shared PG writer
        // pool (MVCC handles concurrency); the DuckDB reader-replica file
        // assertion is retired with the split-brain machinery.
    }

    #[test]
    fn test_normalize_project_code_registry_schema_rejects_legacy_slug_column() {
        let store = create_test_db().unwrap();
        store
            .execute("DROP TABLE soll.ProjectCodeRegistry")
            .unwrap();
        store
            .execute(
                "CREATE TABLE soll.ProjectCodeRegistry (
                project_code VARCHAR PRIMARY KEY,
                project_slug VARCHAR,
                project_path VARCHAR,
                project_name VARCHAR
            )",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.ProjectCodeRegistry (project_code, project_slug, project_path, project_name)
                 VALUES ('AXO', 'axon', '/home/dstadel/projects/axon', 'Axon')",
            )
            .unwrap();

        let err = store.normalize_project_code_registry_schema().unwrap_err();
        assert!(err
            .to_string()
            .contains("Legacy soll.ProjectCodeRegistry schema detected"));
    }

    #[test]
    fn test_normalize_revision_preview_schema_accepts_canonical_schema() {
        let store = create_test_db().unwrap();
        store.normalize_revision_preview_schema().unwrap();
    }

    #[test]
    fn test_normalize_revision_preview_schema_rejects_legacy_slug_column() {
        let store = create_test_db().unwrap();
        store.execute("DROP TABLE soll.RevisionPreview").unwrap();
        store
            .execute(
                "CREATE TABLE soll.RevisionPreview (
                preview_id VARCHAR PRIMARY KEY,
                author VARCHAR,
                project_slug VARCHAR,
                payload VARCHAR,
                created_at BIGINT
            )",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.RevisionPreview (preview_id, author, project_slug, payload, created_at)
                 VALUES ('PRV-HYD-002', 'unknown', 'HydraDB', '{}', 42)",
            )
            .unwrap();

        let err = store.normalize_revision_preview_schema().unwrap_err();
        assert!(err
            .to_string()
            .contains("Legacy soll.RevisionPreview schema detected"));
    }

    // REQ-AXO-901653 slice-5c — `test_soft_invalidate_embedding_state_*` deleted ;
    // exercised legacy `soft_invalidate_embedding_state` + public.File/FileVectorizationQueue.

    #[test]
    fn startup_vector_backfill_limit_keeps_vector_startup_bounded_by_graph_ready_stock() {
        assert_eq!(startup_vector_backfill_limit(0, 0), 0);
        assert_eq!(startup_vector_backfill_limit(0, 1), 1);
        assert_eq!(startup_vector_backfill_limit(1, 1), 1);
        assert_eq!(
            startup_vector_backfill_limit(0, 512),
            STARTUP_SEMANTIC_BACKFILL_FLOOR
        );
        assert_eq!(
            startup_vector_backfill_limit(512, 512),
            STARTUP_SEMANTIC_BACKFILL_FLOOR
        );
    }

    // REQ-AXO-066 Phase 1 (DEC-AXO-064 Option A): two projects coexist in the
    // shared SOLL store and remain semantically isolated under project_code
    // filters; the composite multi-tenant indexes are present after bootstrap.
    #[test]
    fn test_two_projects_are_semantically_isolated_in_soll() {
        let store = create_test_db().unwrap();

        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('REQ-AXO-90001', 'Requirement', 'AXO', 'AXO smoke', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('REQ-BKS-90001', 'Requirement', 'BKS', 'BKS smoke', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('CPT-AXO-90001', 'Concept', 'AXO', 'AXO concept', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('CPT-BKS-90001', 'Concept', 'BKS', 'BKS concept', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code)
                 VALUES ('REQ-AXO-90001', 'CPT-AXO-90001', 'BELONGS_TO', '{}', 'AXO')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code)
                 VALUES ('REQ-BKS-90001', 'CPT-BKS-90001', 'BELONGS_TO', '{}', 'BKS')",
            )
            .unwrap();

        let axo_nodes = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'AXO' AND id LIKE '%-90001'",
            )
            .unwrap();
        let bks_nodes = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'BKS' AND id LIKE '%-90001'",
            )
            .unwrap();
        assert_eq!(axo_nodes, 2, "AXO scope must see exactly 2 seeded nodes");
        assert_eq!(bks_nodes, 2, "BKS scope must see exactly 2 seeded nodes");

        // Cross-project leak: AXO scope must never expose BKS rows.
        let axo_seeing_bks = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'AXO' AND id LIKE '%-BKS-%'",
            )
            .unwrap();
        let bks_seeing_axo = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'BKS' AND id LIKE '%-AXO-%'",
            )
            .unwrap();
        assert_eq!(axo_seeing_bks, 0, "AXO scope leaked BKS rows");
        assert_eq!(bks_seeing_axo, 0, "BKS scope leaked AXO rows");

        // Edge.project_code denormalization works under per-tenant filter.
        let axo_edges = store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE project_code = 'AXO' AND source_id = 'REQ-AXO-90001'",
            )
            .unwrap();
        let bks_edges = store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE project_code = 'BKS' AND source_id = 'REQ-BKS-90001'",
            )
            .unwrap();
        assert_eq!(axo_edges, 1);
        assert_eq!(bks_edges, 1);

        // Composite indexes from REQ-AXO-066 Phase 1 are registered by bootstrap.
        let raw = store
            .query_json(
                "SELECT index_name FROM duckdb_indexes()
                 WHERE schema_name = 'main'
                   AND index_name IN (
                       'soll_node_project_id_idx',
                       'soll_edge_project_source_idx',
                       'soll_edge_project_target_idx',
                       'soll_mcp_job_project_status_idx',
                       'soll_revision_project_idx',
                       'soll_revision_change_project_idx',
                       'symbol_project_id_idx',
                       'calls_project_source_idx',
                       'file_project_path_idx'
                   )
                 ORDER BY index_name",
            )
            .unwrap();
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap();
        let names: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();
        for expected in [
            "calls_project_source_idx",
            "file_project_path_idx",
            "soll_edge_project_source_idx",
            "soll_edge_project_target_idx",
            "soll_mcp_job_project_status_idx",
            "soll_node_project_id_idx",
            "soll_revision_change_project_idx",
            "soll_revision_project_idx",
            "symbol_project_id_idx",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing composite multi-tenant index `{expected}`; present: {names:?}"
            );
        }
    }
}

#[allow(dead_code)]
fn parse_prefixed_entity_id(value: &str) -> Option<(&str, &str, u64)> {
    let trimmed = value.trim();
    let mut parts = trimmed.splitn(3, '-');
    let prefix = parts.next()?;
    let project = parts.next()?;
    let number_str = parts.next()?;
    let number = number_str.parse::<u64>().ok()?;
    Some((prefix, project, number))
}

#[allow(dead_code)]
fn split_prefixed_display_name(value: &str) -> Option<(String, String)> {
    let (id_part, name_part) = value.split_once(':')?;
    let id = id_part.trim();
    parse_prefixed_entity_id(id)?;
    Some((id.to_string(), name_part.trim().to_string()))
}

#[cfg(test)]
mod tests;
