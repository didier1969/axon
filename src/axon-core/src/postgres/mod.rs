// MIL-AXO-015 P1+P2: PostgreSQL backend scaffold + DDL generator.
//
// This module is the entry point for the PostgreSQL replacement of the
// DuckDB-based GraphStore (DEC-AXO-075).
//
// Surface:
//   - `build_pool` (P1): deadpool_postgres::Pool from a DATABASE_URL
//     (or AXON_LIVE_DATABASE_URL / AXON_DEV_DATABASE_URL).
//   - `smoke_check` (P1): loads `age` + `vector` extensions and reports
//     versions, so a misconfigured client database fails fast at brain
//     bootstrap.
//   - `ddl::generate_global_schema` (P2; expanded 2026-05-08 to provision
//     multi-project IST tables + axon_runtime telemetry + AGE labels).
//   - `ddl::generate_project_schema(project_code)` (post-CPT-AXO-039
//     supersedure 2026-05-08): no-op DDL, kept for API stability +
//     project_code injection guard.
//   - `vector::{vector_literal, upsert_chunk_embedding_sql,
//     cosine_ann_where_order_limit}` (P4): pgvector helpers, all
//     multi-project after the CPT-AXO-039 supersedure.
//   - `age::{cypher_merge_vertex, cypher_merge_edge, cypher_query,
//     cypher_props_literal}` (option B.2 foundation 2026-05-08):
//     AGE Cypher writer + reader helpers, validate identifiers and
//     escape property strings so the heredoc cannot be terminated.
//   - `seed::{apply_seed, load_seed_if_needed, SeedDocument}` (P5):
//     SOLL bootstrap loader for empty PG instances.

pub mod age;
pub mod bulk_writer;
pub mod ddl;
pub mod seed;
pub mod vector;

use std::time::Duration;

use anyhow::{Context, Result};
use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;
use tracing::{debug, info};

/// Default connection acquisition timeout. Production runs with sane
/// defaults; tests override via env.
const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum number of connections in the pool. Tuned conservatively for
/// single-host dev; scaled per-deployment via PG_MAX_CONNECTIONS env.
const DEFAULT_MAX_CONNECTIONS: usize = 16;

#[derive(Debug, Clone, Copy)]
pub enum AxonInstance {
    Live,
    Dev,
}

/// Thin wrapper that selects the appropriate `DATABASE_URL` for the
/// current AxonRuntimeMode. Falls back to a generic `DATABASE_URL` env
/// var so plain `cargo test` works without devenv-specific env vars set.
pub fn database_url_for(instance: AxonInstance) -> Result<String> {
    let primary = match instance {
        AxonInstance::Live => "AXON_LIVE_DATABASE_URL",
        AxonInstance::Dev => "AXON_DEV_DATABASE_URL",
    };
    if let Ok(url) = std::env::var(primary) {
        if !url.is_empty() {
            return Ok(url);
        }
    }
    std::env::var("DATABASE_URL").with_context(|| {
        format!(
            "neither {primary} nor DATABASE_URL is set; cannot resolve PostgreSQL connection string"
        )
    })
}

/// Build a connection pool against the given URL. Honors
/// `PG_MAX_CONNECTIONS` and `PG_ACQUIRE_TIMEOUT_MS` env overrides.
pub async fn build_pool(database_url: &str) -> Result<Pool> {
    let max_connections = std::env::var("PG_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_CONNECTIONS);
    let acquire_timeout = std::env::var("PG_ACQUIRE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_ACQUIRE_TIMEOUT);

    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });

    let pool = cfg
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .with_context(|| format!("failed to create pool for {database_url}"))?;

    // Resize to honor max_connections (deadpool defaults to 1 worker).
    pool.resize(max_connections);

    // Probe one connection now so misconfigured URLs fail fast.
    let probe = tokio::time::timeout(acquire_timeout, pool.get())
        .await
        .with_context(|| format!("acquire timeout {:?} expired", acquire_timeout))?
        .with_context(|| format!("failed to acquire initial connection from {database_url}"))?;
    drop(probe);

    info!(
        max_connections,
        "postgres pool established (acquire_timeout_ms={})",
        acquire_timeout.as_millis()
    );
    Ok(pool)
}

/// Smoke-check the connected PostgreSQL: required version + extensions
/// must be loadable. Used at brain bootstrap to fail fast if the operator
/// or client provided a misconfigured database.
pub async fn smoke_check(pool: &Pool) -> Result<SmokeReport> {
    let conn = pool
        .get()
        .await
        .context("smoke_check could not acquire a connection")?;

    let row = conn
        .query_one("SELECT version()", &[])
        .await
        .context("server_version probe failed")?;
    let server_version: String = row.get(0);

    // Apache AGE: ensure CREATE EXTENSION succeeds (idempotent if already
    // installed). CPT-AXO-040.
    conn.batch_execute("CREATE EXTENSION IF NOT EXISTS age")
        .await
        .context("CREATE EXTENSION age failed; install Apache AGE in this database")?;

    // pgvector: same idempotent check. CPT-AXO-041.
    conn.batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
        .await
        .context("CREATE EXTENSION vector failed; install pgvector in this database")?;

    let age_row = conn
        .query_opt(
            "SELECT extversion FROM pg_extension WHERE extname = $1",
            &[&"age"],
        )
        .await
        .context("age version probe failed")?;
    let age_version = age_row.map(|r| r.get::<_, String>(0));

    let vector_row = conn
        .query_opt(
            "SELECT extversion FROM pg_extension WHERE extname = $1",
            &[&"vector"],
        )
        .await
        .context("vector version probe failed")?;
    let vector_version = vector_row.map(|r| r.get::<_, String>(0));

    debug!(
        ?age_version,
        ?vector_version,
        "extensions present after smoke check"
    );

    Ok(SmokeReport {
        server_version,
        age_version,
        vector_version,
    })
}

#[derive(Debug, Clone)]
pub struct SmokeReport {
    pub server_version: String,
    pub age_version: Option<String>,
    pub vector_version: Option<String>,
}

impl SmokeReport {
    pub fn is_complete(&self) -> bool {
        self.age_version.is_some() && self.vector_version.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Global lock for the env-var-mutating tests below. Cargo runs tests
    /// in parallel by default, but std::env is process-global, so two
    /// tests that touch the same vars race and produce flaky failures.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard: snapshots the listed env vars on construction, restores
    /// them (set or unset) on Drop. Holds the ENV_LOCK so concurrent
    /// tests never observe each other's mid-run state.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        snapshots: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(vars: &[&'static str]) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let snapshots = vars.iter().map(|v| (*v, std::env::var(*v).ok())).collect();
            Self {
                _lock: lock,
                snapshots,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.snapshots {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn database_url_falls_back_to_generic_var() {
        let _g = EnvGuard::new(&["AXON_LIVE_DATABASE_URL", "DATABASE_URL"]);
        std::env::remove_var("AXON_LIVE_DATABASE_URL");
        std::env::set_var("DATABASE_URL", "postgres://fallback/db");
        let url = database_url_for(AxonInstance::Live).unwrap();
        assert_eq!(url, "postgres://fallback/db");
    }

    #[test]
    fn database_url_prefers_instance_specific_var() {
        let _g = EnvGuard::new(&["AXON_LIVE_DATABASE_URL", "DATABASE_URL"]);
        std::env::set_var("AXON_LIVE_DATABASE_URL", "postgres://live/db");
        std::env::set_var("DATABASE_URL", "postgres://generic/db");
        let url = database_url_for(AxonInstance::Live).unwrap();
        assert_eq!(url, "postgres://live/db");
    }

    #[test]
    fn database_url_errors_when_unset() {
        let _g = EnvGuard::new(&["AXON_DEV_DATABASE_URL", "DATABASE_URL"]);
        std::env::remove_var("AXON_DEV_DATABASE_URL");
        std::env::remove_var("DATABASE_URL");
        let result = database_url_for(AxonInstance::Dev);
        assert!(result.is_err(), "expected error when no env var is set");
    }

    #[test]
    fn smoke_report_completeness_check() {
        let r = SmokeReport {
            server_version: "PostgreSQL 17".to_string(),
            age_version: Some("1.5.0".to_string()),
            vector_version: Some("0.8.0".to_string()),
        };
        assert!(r.is_complete());
        let partial = SmokeReport {
            server_version: "PostgreSQL 17".to_string(),
            age_version: Some("1.5.0".to_string()),
            vector_version: None,
        };
        assert!(!partial.is_complete());
    }
}
