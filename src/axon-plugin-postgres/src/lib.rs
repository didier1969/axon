//! MIL-AXO-015 P3 — PostgreSQL replacement for axon-plugin-duckdb.
//!
//! Mirrors the C-FFI surface of axon-plugin-duckdb so axon-core can adopt
//! it through the existing libloading-based plugin discovery
//! (`graph_bootstrap.rs:907`). Function symbols are renamed to a `pg_*`
//! prefix so both plugins can coexist during the migration window.
//!
//! Design rules:
//! - Sync FFI boundary, async tokio internals: each entry point hops
//!   through a `tokio::runtime::Runtime` that the `PgPluginContext`
//!   owns. Callers must invoke from a thread that is NOT already
//!   executing inside another tokio runtime, exactly as the indexer /
//!   brain worker pools do today for axon-plugin-duckdb.
//! - Error envelope contract (REQ-AXO-129) preserved: invalid SQL
//!   returns a JSON object `{"_axon_plugin_error", "stage",
//!   "sql_excerpt"}`. A genuine empty result is still `[]`.
//! - Per-connection session setup: when a `schema_search_path` is
//!   configured AND/OR the AGE extension is detected at init time,
//!   every connection acquired from the pool emits the equivalent of
//!   `LOAD 'age'; SET search_path TO <schema>, ag_catalog, public`
//!   before running the user SQL. This is how the per-project
//!   namespace from CPT-AXO-039 surfaces inside dynamic SQL and how
//!   `cypher()` / `agtype` resolve unqualified per Apache AGE README
//!   (CPT-AXO-040).
//!
//! Out of scope (deferred to subsequent P3 slices):
//! - Vector parameter binding for pgvector (P4).
//! - axon-core graph_bootstrap wiring (separate commit).

use std::ffi::{c_char, CStr, CString};

use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime as DpRuntime};
use tokio::runtime::{Builder as RtBuilder, Runtime};
use tokio_postgres::types::ToSql;
use tokio_postgres::NoTls;

/// Plugin-owned state. One instance per PG-backed Axon connection.
pub struct PgPluginContext {
    pool: Pool,
    runtime: Runtime,
    schema_search_path: Option<String>,
    /// Detected at init time. When true, every connection acquired
    /// from the pool runs `LOAD 'age'` and prepends `ag_catalog` to
    /// `search_path` so callers can write unqualified Cypher via
    /// `cypher()`. Disabled automatically when the AGE extension is
    /// not installed in the database.
    age_enabled: bool,
}

fn plugin_trace_enabled() -> bool {
    std::env::var("AXON_PG_PLUGIN_TRACE")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// REQ-AXO-129 — plugin error envelope. Keeps the contract identical
/// to axon-plugin-duckdb so callers (`graph_query::query_on_ctx`)
/// dispatch on the leading JSON character.
fn plugin_error_envelope(stage: &str, sql: &str, err: impl std::fmt::Display) -> *mut c_char {
    let payload = serde_json::json!({
        "_axon_plugin_error": format!("{stage}: {err}"),
        "stage": stage,
        "sql_excerpt": sql.chars().take(240).collect::<String>(),
    });
    CString::new(payload.to_string()).unwrap().into_raw()
}

/// Specialised envelope for `tokio_postgres::Error`: drills into the
/// `DbError` payload (when present) so the caller sees the SQLSTATE
/// message + position rather than the generic `db error` Display.
fn plugin_db_error_envelope(
    stage: &str,
    sql: &str,
    err: &tokio_postgres::Error,
) -> *mut c_char {
    let display = err.to_string();
    let mut detail = serde_json::json!({});
    if let Some(db) = err.as_db_error() {
        detail = serde_json::json!({
            "code": db.code().code(),
            "severity": db.severity(),
            "message": db.message(),
            "detail": db.detail(),
            "hint": db.hint(),
            "position": db.position().map(|p| format!("{p:?}")),
        });
    }
    let payload = serde_json::json!({
        "_axon_plugin_error": format!("{stage}: {display}"),
        "stage": stage,
        "sql_excerpt": sql.chars().take(240).collect::<String>(),
        "pg_error": detail,
    });
    CString::new(payload.to_string()).unwrap().into_raw()
}

fn empty_array_cstr() -> *mut c_char {
    CString::new("[]").unwrap().into_raw()
}

/// Resolve the per-statement timeout (milliseconds) from the env var
/// `AXON_PG_STATEMENT_TIMEOUT_MS`, defaulting to 30000 (30s). The value
/// 0 disables the timeout. REQ-AXO-91494 — surfaces planner stalls and
/// hash-agg/JOIN pathologies as a structured PG error envelope
/// (`statement_timeout` SQLSTATE 57014) instead of an indistinguishable
/// silent `[]`.
fn statement_timeout_ms() -> u64 {
    std::env::var("AXON_PG_STATEMENT_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30_000)
}

/// Build the per-connection session-setup SQL, given an optional
/// validated schema name and whether AGE is loaded in this database.
///
/// The ordering rules:
/// - When AGE is enabled, `LOAD 'age'` MUST run before any cypher()
///   call; deadpool may hand out a freshly-recycled connection that
///   has lost the load.
/// - `ag_catalog` MUST be on `search_path` so `cypher()` and `agtype`
///   operators resolve unqualified, per AGE README.
/// - The project schema goes FIRST so unqualified table names
///   (e.g. `File`) resolve to `<schema>.File` exactly as the duckdb
///   plugin does today.
/// - REQ-AXO-91494 : `SET statement_timeout` ALWAYS runs (independent
///   of schema/AGE) so silent planner stalls surface as errors.
fn build_session_setup_sql(schema: Option<&str>, age_enabled: bool) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if age_enabled {
        parts.push("LOAD 'age'".to_string());
    }
    let path: Vec<&str> = match (schema, age_enabled) {
        (Some(s), true) => vec![s, "ag_catalog", "public"],
        (Some(s), false) => vec![s, "public"],
        (None, true) => vec!["ag_catalog", "public"],
        (None, false) => vec![],
    };
    if !path.is_empty() {
        parts.push(format!("SET search_path TO {}", path.join(", ")));
    }
    let timeout_ms = statement_timeout_ms();
    if timeout_ms > 0 {
        parts.push(format!("SET statement_timeout TO {timeout_ms}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

/// Run session-setup statements on a freshly acquired connection.
async fn apply_session_setup(
    conn: &deadpool_postgres::Client,
    schema: &Option<String>,
    age_enabled: bool,
) -> Result<(), tokio_postgres::Error> {
    if let Some(sql) = build_session_setup_sql(schema.as_deref(), age_enabled) {
        conn.batch_execute(&sql).await?;
    }
    Ok(())
}

/// Probe the connected database for the AGE extension. Used at init
/// to flip `age_enabled` on the context. Returns `false` on any error
/// (treats AGE as unavailable rather than panicking the plugin).
async fn probe_age_installed(pool: &Pool) -> bool {
    let conn = match pool.get().await {
        Ok(c) => c,
        Err(_) => return false,
    };
    match conn
        .query_opt(
            "SELECT 1 FROM pg_extension WHERE extname = $1",
            &[&"age"],
        )
        .await
    {
        Ok(Some(_)) => true,
        _ => false,
    }
}

/// `[a-zA-Z0-9_]{1,64}` — the schema check mirrors
/// `axon-core::postgres::ddl::schema_name_for` but stays self-contained
/// so this crate carries no path dep on axon-core.
fn validate_schema_identifier(s: &str) -> Result<String, &'static str> {
    if s.is_empty() {
        return Err("schema is empty");
    }
    if s.len() > 64 {
        return Err("schema is longer than 64 chars");
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("schema contains characters outside [a-zA-Z0-9_]");
    }
    Ok(s.to_string())
}

/// Initialise the plugin against a `DATABASE_URL`. `schema` may be a
/// null pointer (skip `SET search_path`) or the empty string (treated as
/// null); otherwise it is validated to keep the eventual interpolation
/// injection-free.
///
/// # Safety
///
/// `database_url` must be a valid C string. `schema` may be null.
#[no_mangle]
pub unsafe extern "C" fn pg_init_db(
    database_url: *const c_char,
    schema: *const c_char,
) -> *mut PgPluginContext {
    if database_url.is_null() {
        return std::ptr::null_mut();
    }
    let url_str = match CStr::from_ptr(database_url).to_str() {
        Ok(s) if !s.is_empty() => s.to_string(),
        _ => return std::ptr::null_mut(),
    };

    let schema_search_path = if schema.is_null() {
        None
    } else {
        match CStr::from_ptr(schema).to_str() {
            Ok(s) if !s.is_empty() => match validate_schema_identifier(s) {
                Ok(v) => Some(v),
                Err(reason) => {
                    eprintln!("[pg_init_db] rejected schema {s:?}: {reason}");
                    return std::ptr::null_mut();
                }
            },
            _ => None,
        }
    };

    if plugin_trace_enabled() {
        eprintln!(
            "[pg_init_db] connecting (schema={:?})",
            schema_search_path
        );
    }

    let runtime = match RtBuilder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("axon-pg-plugin")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[pg_init_db] tokio runtime build failed: {e}");
            return std::ptr::null_mut();
        }
    };

    let pool_result: Result<Pool, deadpool_postgres::CreatePoolError> = runtime.block_on(async {
        let mut cfg = Config::new();
        cfg.url = Some(url_str.clone());
        cfg.manager = Some(ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        });
        cfg.create_pool(Some(DpRuntime::Tokio1), NoTls)
    });
    let pool = match pool_result {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[pg_init_db] pool creation failed: {e}");
            return std::ptr::null_mut();
        }
    };

    // Probe one connection so misconfigured URLs fail fast at boot,
    // and detect whether the AGE extension is installed so subsequent
    // session setups can opt in.
    let probe = runtime.block_on(async { pool.get().await });
    if let Err(e) = probe {
        eprintln!("[pg_init_db] probe connection failed: {e}");
        return std::ptr::null_mut();
    }
    let age_enabled = runtime.block_on(probe_age_installed(&pool));
    if plugin_trace_enabled() {
        eprintln!("[pg_init_db] age_enabled={age_enabled}");
    }

    Box::into_raw(Box::new(PgPluginContext {
        pool,
        runtime,
        schema_search_path,
        age_enabled,
    }))
}

/// Free a string previously returned by `pg_query_json`.
///
/// # Safety
///
/// Must be called at most once per `pg_query_json` return. The pointer
/// must originate from this crate.
#[no_mangle]
pub unsafe extern "C" fn pg_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}

/// Tear down the plugin context, releasing the pool and shutting down
/// the runtime.
///
/// # Safety
///
/// Must be called at most once per `pg_init_db`. The pointer must
/// originate from this crate.
#[no_mangle]
pub unsafe extern "C" fn pg_close_db(ctx: *mut PgPluginContext) {
    if !ctx.is_null() {
        let _ = Box::from_raw(ctx);
    }
}

/// Shim with the same FFI shape as `axon-plugin-duckdb::duckdb_init_db`
/// (path: *const c_char, read_only: bool) -> *mut c_void.
///
/// This is the entry point axon-core's `PluginSymbols::resolve_postgres`
/// resolves under MIL-AXO-015 P3 slice 3b so the consumer code path can
/// stay backend-agnostic. The first argument is reinterpreted as a
/// `DATABASE_URL` (file paths under duckdb). The `_read_only` flag is
/// ignored — PostgreSQL handles concurrency at the server layer, so we
/// surface the same pool to both reader and writer call sites.
///
/// The schema search_path is left null at this layer; axon-core injects
/// it later by issuing a `SET search_path` on the acquired connection
/// once the project_code is known. This keeps slice 3b additive: the
/// shim does not depend on slice 3a's PluginSymbols-on-LatticePool
/// abstraction.
///
/// # Safety
///
/// `database_url` must be a valid C string. The bool argument is
/// ignored; pass either value.
#[no_mangle]
pub unsafe extern "C" fn pg_init_db_compat(
    database_url: *const c_char,
    _read_only: bool,
) -> *mut PgPluginContext {
    pg_init_db(database_url, std::ptr::null())
}

/// Execute a SQL batch (semicolon-separated allowed) without binding
/// parameters. Returns `true` on success.
///
/// # Safety
///
/// `ctx` must originate from `pg_init_db`. `sql` must be a valid C
/// string.
#[no_mangle]
pub unsafe extern "C" fn pg_execute(ctx: *mut PgPluginContext, sql: *const c_char) -> bool {
    if ctx.is_null() || sql.is_null() {
        return false;
    }
    let sql_str = match CStr::from_ptr(sql).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    if plugin_trace_enabled() {
        eprintln!("[pg_execute] {sql_str}");
    }
    let ctx_ref = &*ctx;

    ctx_ref.runtime.block_on(async {
        let conn = match ctx_ref.pool.get().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[pg_execute] pool acquire failed: {e}");
                return false;
            }
        };
        if let Err(e) = apply_session_setup(&conn, &ctx_ref.schema_search_path, ctx_ref.age_enabled).await {
            eprintln!("[pg_execute] set search_path failed: {e}");
            return false;
        }
        match conn.batch_execute(sql_str).await {
            Ok(_) => true,
            Err(e) => {
                eprintln!("[pg_execute] {e} | {sql_str}");
                false
            }
        }
    })
}

/// Execute a parameterised statement. `params_json` must be a JSON
/// array; supported element types are string, integer, float, bool, and
/// null. The contract matches axon-plugin-duckdb's
/// `duckdb_execute_param`.
///
/// # Safety
///
/// `ctx` must originate from `pg_init_db`. `sql` and `params_json` must
/// be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn pg_execute_param(
    ctx: *mut PgPluginContext,
    sql: *const c_char,
    params_json: *const c_char,
) -> bool {
    if ctx.is_null() || sql.is_null() || params_json.is_null() {
        return false;
    }
    let sql_str = match CStr::from_ptr(sql).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let params_str = match CStr::from_ptr(params_json).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    if plugin_trace_enabled() {
        eprintln!("[pg_execute_param] {sql_str} | {params_str}");
    }

    let owned = match decode_params(params_str) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[pg_execute_param] params decode failed: {e}");
            return false;
        }
    };

    let ctx_ref = &*ctx;
    ctx_ref.runtime.block_on(async {
        let conn = match ctx_ref.pool.get().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[pg_execute_param] pool acquire failed: {e}");
                return false;
            }
        };
        if let Err(e) = apply_session_setup(&conn, &ctx_ref.schema_search_path, ctx_ref.age_enabled).await {
            eprintln!("[pg_execute_param] set search_path failed: {e}");
            return false;
        }
        let refs: Vec<&(dyn ToSql + Sync)> =
            owned.iter().map(|p| p.as_to_sql()).collect();
        match conn.execute(sql_str, &refs[..]).await {
            Ok(_) => true,
            Err(e) => {
                eprintln!("[pg_execute_param] {e} | {sql_str}");
                false
            }
        }
    })
}

/// Run a `SELECT count(*)` style query and return the first column of
/// the first row coerced to `i64`. Returns `-1` on any error so the
/// FFI signature stays identical to `duckdb_query_count`.
///
/// # Safety
///
/// See `pg_execute`.
#[no_mangle]
pub unsafe extern "C" fn pg_query_count(ctx: *mut PgPluginContext, sql: *const c_char) -> i64 {
    if ctx.is_null() || sql.is_null() {
        return -1;
    }
    let sql_str = match CStr::from_ptr(sql).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    if plugin_trace_enabled() {
        eprintln!("[pg_query_count] {sql_str}");
    }
    let ctx_ref = &*ctx;
    ctx_ref.runtime.block_on(async {
        let conn = match ctx_ref.pool.get().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[pg_query_count] pool acquire failed: {e}");
                return -1;
            }
        };
        if let Err(e) = apply_session_setup(&conn, &ctx_ref.schema_search_path, ctx_ref.age_enabled).await {
            eprintln!("[pg_query_count] set search_path failed: {e}");
            return -1;
        }
        match conn.query_one(sql_str, &[]).await {
            Ok(row) => match row.try_get::<_, i64>(0) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[pg_query_count] column 0 not i64: {e}");
                    -1
                }
            },
            Err(e) => {
                eprintln!("[pg_query_count] {e} | {sql_str}");
                -1
            }
        }
    })
}

/// Run an arbitrary SELECT and return the result set as a JSON array of
/// arrays of strings (matching `duckdb_query_json`). Non-SELECT input
/// runs as a side-effecting statement and yields literal `[]`. Errors
/// are surfaced via the REQ-AXO-129 envelope.
///
/// # Safety
///
/// See `pg_execute`.
#[no_mangle]
pub unsafe extern "C" fn pg_query_json(
    ctx: *mut PgPluginContext,
    sql: *const c_char,
) -> *mut c_char {
    if ctx.is_null() || sql.is_null() {
        return empty_array_cstr();
    }
    let sql_str = match CStr::from_ptr(sql).to_str() {
        Ok(s) => s,
        Err(_) => return empty_array_cstr(),
    };
    if plugin_trace_enabled() {
        eprintln!("[pg_query_json] {sql_str}");
    }
    let ctx_ref = &*ctx;

    let leading = sql_str.trim_start().to_lowercase();
    let returns_rows = leading.starts_with("select")
        || leading.starts_with("with")
        || leading.starts_with("show")
        || leading.starts_with("table ")
        || leading.starts_with("values")
        || sql_str.to_lowercase().contains(" returning ");

    ctx_ref.runtime.block_on(async {
        let conn = match ctx_ref.pool.get().await {
            Ok(c) => c,
            Err(e) => return plugin_error_envelope("acquire", sql_str, e),
        };
        if let Err(e) = apply_session_setup(&conn, &ctx_ref.schema_search_path, ctx_ref.age_enabled).await {
            return plugin_db_error_envelope("set_search_path", sql_str, &e);
        }

        if !returns_rows {
            return match conn.batch_execute(sql_str).await {
                Ok(_) => empty_array_cstr(),
                Err(e) => plugin_db_error_envelope("execute", sql_str, &e),
            };
        }

        match conn.query(sql_str, &[]).await {
            Ok(rows) => {
                let mut out: Vec<Vec<String>> = Vec::with_capacity(rows.len());
                for row in &rows {
                    let mut rendered = Vec::with_capacity(row.len());
                    for col in 0..row.len() {
                        rendered.push(render_pg_value(row, col));
                    }
                    out.push(rendered);
                }
                let json = serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string());
                CString::new(json).unwrap().into_raw()
            }
            Err(e) => plugin_db_error_envelope("query", sql_str, &e),
        }
    })
}

/// Render a single value out of a `tokio_postgres::Row` as a string,
/// matching the duckdb plugin's per-column shape (each column collapses
/// to a `String`; arrays/JSON serialise to JSON text).
fn render_pg_value(row: &tokio_postgres::Row, col: usize) -> String {
    use tokio_postgres::types::Type;
    let ty = row.columns()[col].type_().clone();
    if let Ok(opt) = row.try_get::<_, Option<&str>>(col) {
        match opt {
            Some(s) => return s.to_string(),
            None => return "null".to_string(),
        }
    }
    if ty == Type::INT2 {
        if let Ok(v) = row.try_get::<_, Option<i16>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::INT4 {
        if let Ok(v) = row.try_get::<_, Option<i32>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::INT8 {
        if let Ok(v) = row.try_get::<_, Option<i64>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::FLOAT4 {
        if let Ok(v) = row.try_get::<_, Option<f32>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::FLOAT8 {
        if let Ok(v) = row.try_get::<_, Option<f64>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::BOOL {
        if let Ok(v) = row.try_get::<_, Option<bool>>(col) {
            return v.map(|b| b.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::JSON || ty == Type::JSONB {
        if let Ok(v) = row.try_get::<_, Option<serde_json::Value>>(col) {
            return v.map(|j| j.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    // Last-resort fallback. We do not know the type, so emit a marker
    // string rather than panicking; callers see this as a hint to add
    // a typed branch above.
    format!("<unsupported type {}>", ty.name())
}

/// Owned representation of a JSON-decoded SQL parameter. Lives long
/// enough that we can hand `&dyn ToSql` references into tokio-postgres.
enum OwnedParam {
    Null,
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl OwnedParam {
    fn as_to_sql(&self) -> &(dyn ToSql + Sync) {
        match self {
            OwnedParam::Null => &NONE_STR,
            OwnedParam::Text(s) => s,
            OwnedParam::Int(i) => i,
            OwnedParam::Float(f) => f,
            OwnedParam::Bool(b) => b,
        }
    }
}

// `Option::<&str>::None` upcasts to a NULL-typed parameter.
const NONE_STR: Option<&str> = None;

fn decode_params(json_str: &str) -> Result<Vec<OwnedParam>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid params JSON: {e}"))?;
    let arr = match v {
        serde_json::Value::Array(a) => a,
        _ => return Err("params must be a JSON array".to_string()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(match item {
            serde_json::Value::Null => OwnedParam::Null,
            serde_json::Value::String(s) => OwnedParam::Text(s),
            serde_json::Value::Bool(b) => OwnedParam::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    OwnedParam::Int(i)
                } else if let Some(f) = n.as_f64() {
                    OwnedParam::Float(f)
                } else {
                    return Err(format!("number out of range: {n}"));
                }
            }
            other => OwnedParam::Text(other.to_string()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-var mutations across parallel test threads. Each
    // test that flips `AXON_PG_STATEMENT_TIMEOUT_MS` must hold this
    // lock for the whole set+assert+unset cycle.
    fn env_lock() -> &'static Mutex<()> {
        use std::sync::OnceLock;
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn schema_validation_rejects_injection() {
        assert!(validate_schema_identifier("axo").is_ok());
        assert!(validate_schema_identifier("axo_42").is_ok());
        assert!(validate_schema_identifier("").is_err());
        assert!(validate_schema_identifier("axo; DROP TABLE x;--").is_err());
        assert!(validate_schema_identifier("axo space").is_err());
        assert!(validate_schema_identifier("axo'").is_err());
        let long = "a".repeat(65);
        assert!(validate_schema_identifier(&long).is_err());
    }

    #[test]
    fn decode_params_supports_basic_types() {
        let params = decode_params(r#"["s", 42, 3.14, true, null]"#).expect("decode");
        assert_eq!(params.len(), 5);
        assert!(matches!(params[0], OwnedParam::Text(ref s) if s == "s"));
        assert!(matches!(params[1], OwnedParam::Int(42)));
        match params[2] {
            OwnedParam::Float(f) => assert!((f - 3.14_f64).abs() < 1e-9),
            _ => panic!("expected float"),
        }
        assert!(matches!(params[3], OwnedParam::Bool(true)));
        assert!(matches!(params[4], OwnedParam::Null));
    }

    #[test]
    fn decode_params_rejects_non_array() {
        assert!(decode_params(r#"{"foo": 1}"#).is_err());
        assert!(decode_params(r#"42"#).is_err());
        assert!(decode_params("not json").is_err());
    }

    #[test]
    fn error_envelope_truncates_long_sql() {
        let long_sql = "SELECT ".to_string() + &"x".repeat(2000);
        let raw =
            unsafe { CString::from_raw(plugin_error_envelope("prepare", &long_sql, "boom")) };
        let json = raw.to_str().expect("utf-8");
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["stage"], "prepare");
        assert!(parsed["_axon_plugin_error"]
            .as_str()
            .unwrap()
            .contains("prepare: boom"));
        let excerpt = parsed["sql_excerpt"].as_str().unwrap();
        assert!(excerpt.len() <= 240);
        assert!(excerpt.starts_with("SELECT "));
    }

    #[test]
    fn empty_array_round_trip_is_well_formed() {
        let raw = empty_array_cstr();
        let cstr = unsafe { CString::from_raw(raw) };
        assert_eq!(cstr.to_str().unwrap(), "[]");
    }

    /// The duckdb plugin's signature returned a non-null pointer even
    /// for an invalid path; the PG version follows the same contract by
    /// rejecting an empty/`NULL` URL with a null pointer so callers can
    /// branch on `if !ctx.is_null()`.
    #[test]
    fn pg_init_db_rejects_null_inputs() {
        unsafe {
            assert!(pg_init_db(std::ptr::null(), std::ptr::null()).is_null());
        }
    }

    #[test]
    fn pg_init_db_rejects_invalid_schema() {
        let url = CString::new("postgres://invalid:1/db").unwrap();
        let bad_schema = CString::new("axo;DROP TABLE x").unwrap();
        unsafe {
            assert!(pg_init_db(url.as_ptr(), bad_schema.as_ptr()).is_null());
        }
    }

    #[test]
    fn session_setup_sql_combinations_are_well_formed() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // Lock env var to a deterministic value so the assertions
        // below remain stable regardless of test-process env.
        std::env::set_var("AXON_PG_STATEMENT_TIMEOUT_MS", "30000");

        // No schema, no AGE — still emits the timeout setting (REQ-AXO-91494).
        assert_eq!(
            build_session_setup_sql(None, false).unwrap(),
            "SET statement_timeout TO 30000"
        );

        // Schema only — preserves the duckdb plugin's prior search_path
        // contract, then appends the timeout (REQ-AXO-91494).
        assert_eq!(
            build_session_setup_sql(Some("axo"), false).unwrap(),
            "SET search_path TO axo, public; SET statement_timeout TO 30000"
        );

        // AGE only — `LOAD 'age'` precedes the SET, ag_catalog first
        // so unqualified cypher() / agtype operators resolve.
        assert_eq!(
            build_session_setup_sql(None, true).unwrap(),
            "LOAD 'age'; SET search_path TO ag_catalog, public; SET statement_timeout TO 30000"
        );

        // Schema + AGE — project schema first (so unqualified table
        // names resolve to the project namespace), ag_catalog still
        // visible for cypher() / agtype.
        assert_eq!(
            build_session_setup_sql(Some("axo"), true).unwrap(),
            "LOAD 'age'; SET search_path TO axo, ag_catalog, public; SET statement_timeout TO 30000"
        );

        std::env::remove_var("AXON_PG_STATEMENT_TIMEOUT_MS");
    }

    #[test]
    fn session_setup_omits_timeout_when_env_zero() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // REQ-AXO-91494 — `AXON_PG_STATEMENT_TIMEOUT_MS=0` disables the
        // timeout (test/dev override). With no other setup needed, the
        // builder returns None.
        std::env::set_var("AXON_PG_STATEMENT_TIMEOUT_MS", "0");
        assert_eq!(build_session_setup_sql(None, false), None);
        assert_eq!(
            build_session_setup_sql(Some("axo"), false).unwrap(),
            "SET search_path TO axo, public"
        );
        std::env::remove_var("AXON_PG_STATEMENT_TIMEOUT_MS");
    }
}
