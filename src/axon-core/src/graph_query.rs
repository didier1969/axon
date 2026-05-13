use std::borrow::Cow;
use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::graph::GraphStore;
use crate::runtime_truth_contract::RuntimeFreshnessState;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadFreshness {
    StaleOk,
    FreshPreferred,
    FreshRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadRoute {
    Reader,
    Writer,
}

impl GraphStore {
    fn normalize_attached_soll_query<'a>(&self, query: &'a str) -> Cow<'a, str> {
        // MIL-AXO-015 P3 slice 3d: under PostgreSQL the SOLL layer is
        // a single PG schema (`soll`), not a DuckDB ATTACH'd database
        // with a nested `main` schema. The duckdb-only `soll.X →
        // soll.main.X` rewrite must not run on PG queries; otherwise
        // statements like `CREATE TABLE soll.Registry` get mangled to
        // `CREATE TABLE soll.main.Registry` which has no parser
        // meaning under PG.
        //
        // REQ-AXO-249: under PG, also translate the DuckDB-only JSON
        // helpers MCP tools embed in their queries to native Postgres
        // operators on JSONB columns (soll.Node.metadata is JSONB per
        // postgres/ddl.rs). Without this, `json_extract_string`,
        // `CAST(json_extract(...) AS VARCHAR)`, etc. fail at runtime,
        // breaking soll_validate / soll_work_plan / completeness reads.
        if self.pool.symbols.backend == crate::graph::PluginBackend::Postgres {
            return rewrite_duckdb_json_helpers_for_pg(query);
        }
        if !self.soll_attached || !query.contains("soll.") {
            return Cow::Borrowed(query);
        }

        fn is_identifier_char(byte: u8) -> bool {
            byte.is_ascii_alphanumeric() || byte == b'_'
        }

        let bytes = query.as_bytes();
        let mut cursor = 0usize;
        let mut rewritten = String::with_capacity(query.len() + 32);
        let mut changed = false;

        while let Some(relative) = query[cursor..].find("soll.") {
            let absolute = cursor + relative;
            let preceded_by_identifier =
                absolute > 0 && is_identifier_char(bytes[absolute.saturating_sub(1)]);
            let already_fully_qualified = query[absolute + 5..].starts_with("main.");

            if preceded_by_identifier || already_fully_qualified {
                rewritten.push_str(&query[cursor..absolute + 5]);
                cursor = absolute + 5;
                continue;
            }

            rewritten.push_str(&query[cursor..absolute]);
            rewritten.push_str("soll.main.");
            cursor = absolute + 5;
            changed = true;
        }

        if !changed {
            return Cow::Borrowed(query);
        }

        rewritten.push_str(&query[cursor..]);
        Cow::Owned(rewritten)
    }
}

/// REQ-AXO-249 / MIL-AXO-015 post-promote: translate DuckDB-only SQL
/// patterns embedded in MCP tool queries into PostgreSQL-compatible
/// equivalents.
///
/// **JSONB helpers.** soll.Node.metadata is `JSONB` under PG
/// (postgres/ddl.rs). DuckDB stores the same column as VARCHAR-of-JSON
/// but exposes `json_extract` / `json_extract_string` regardless of
/// column type. Postgres has the operators `->` (returns json/jsonb)
/// and `->>` (returns text):
///
///   • `json_extract_string(col, '$.path')`             → `(col->>'path')`
///   • `CAST(json_extract(col, '$.path') AS VARCHAR)`    → `(col->>'path')`
///   • bare `json_extract(col, '$.path')` (rare here)    → `(col->'path')`
///
/// **3-part SOLL identifiers.** DuckDB's ATTACH'd database exposes the
/// SOLL layer as `soll.main.X` (catalog.schema.table). Postgres has no
/// `main` schema — it's a single `soll` schema, so MCP tools that emit
/// the legacy 3-part name are rewritten to 2-part `soll.X`.
///
/// **Schema-qualified runtime tables.** Tables that DuckDB hosted in
/// the default `main` schema live in `axon_runtime` under PG (per
/// postgres/ddl.rs). The rewriter prepends the schema for the small
/// set of unqualified runtime tables MCP tools still query.
///
/// Limitations:
///   • Only `$.<key>` paths (single-level) are translated for json_extract.
///     Multi-level `$.a.b` would need `(col->'a'->>'b')`. Today no MCP
///     tool uses deeper paths; if one appears, extend the regex.
fn rewrite_duckdb_json_helpers_for_pg(query: &str) -> Cow<'_, str> {
    let touches_json = query.contains("json_extract");
    let touches_soll_main = query.contains("soll.main.");
    let touches_optimizer = query.contains("OptimizerDecisionLog")
        && !query.contains("axon_runtime.OptimizerDecisionLog");
    if !touches_json && !touches_soll_main && !touches_optimizer {
        return Cow::Borrowed(query);
    }

    use std::sync::OnceLock;
    use regex::Regex;
    static CAST_VARCHAR: OnceLock<Regex> = OnceLock::new();
    static EXTRACT_STRING: OnceLock<Regex> = OnceLock::new();
    static EXTRACT_PLAIN: OnceLock<Regex> = OnceLock::new();
    static SOLL_MAIN: OnceLock<Regex> = OnceLock::new();
    static OPTIMIZER_LOG: OnceLock<Regex> = OnceLock::new();
    let cast_varchar = CAST_VARCHAR.get_or_init(|| {
        Regex::new(
            r"(?i)CAST\s*\(\s*json_extract\s*\(\s*([A-Za-z_][A-Za-z0-9_.]*)\s*,\s*'\$\.([A-Za-z_][A-Za-z0-9_]*)'\s*\)\s*AS\s+(?:VARCHAR|TEXT)\s*\)",
        )
        .expect("cast_varchar regex")
    });
    let extract_string = EXTRACT_STRING.get_or_init(|| {
        Regex::new(
            r"(?i)json_extract_string\s*\(\s*([A-Za-z_][A-Za-z0-9_.]*)\s*,\s*'\$\.([A-Za-z_][A-Za-z0-9_]*)'\s*\)",
        )
        .expect("extract_string regex")
    });
    let extract_plain = EXTRACT_PLAIN.get_or_init(|| {
        Regex::new(
            r"(?i)json_extract\s*\(\s*([A-Za-z_][A-Za-z0-9_.]*)\s*,\s*'\$\.([A-Za-z_][A-Za-z0-9_]*)'\s*\)",
        )
        .expect("extract_plain regex")
    });
    let soll_main = SOLL_MAIN.get_or_init(|| {
        Regex::new(r"(?i)\bsoll\.main\.").expect("soll_main regex")
    });
    let optimizer_log = OPTIMIZER_LOG.get_or_init(|| {
        Regex::new(r"(?i)(?P<lead>(?:^|[^.\w]))OptimizerDecisionLog\b")
            .expect("optimizer_log regex")
    });

    // ORDER MATTERS: longer / outer patterns first so CAST(...) is
    // collapsed before the inner json_extract gets rewritten.
    let pass1 = cast_varchar.replace_all(query, "($1->>'$2')");
    let pass2 = extract_string.replace_all(&pass1, "($1->>'$2')");
    let pass3 = extract_plain.replace_all(&pass2, "($1->'$2')");
    let pass4 = soll_main.replace_all(&pass3, "soll.");
    let pass5 = optimizer_log.replace_all(&pass4, "${lead}axon_runtime.OptimizerDecisionLog");
    if pass5 == query {
        Cow::Borrowed(query)
    } else {
        Cow::Owned(pass5.into_owned())
    }
}

impl GraphStore {
    fn reader_only_ist_unavailable_error(&self) -> anyhow::Error {
        let contract = self.reader_snapshot_freshness_contract();
        let reason = contract
            .degraded_reason
            .as_deref()
            .unwrap_or("ist_reader_unavailable");
        anyhow!(
            "IST reader-only access unavailable in split brain mode: {}",
            reason
        )
    }

    fn reader_refresh_request_debounce_ms() -> u64 {
        std::env::var("AXON_READER_REFRESH_REQUEST_DEBOUNCE_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(1_000)
            .clamp(50, 60_000)
    }

    fn reader_refresh_small_lag_epochs() -> u64 {
        std::env::var("AXON_READER_REFRESH_SMALL_LAG_EPOCHS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(32)
            .max(1)
    }

    fn should_request_reader_refresh_for_read(&self, freshness: ReadFreshness, lag: u64) -> bool {
        if lag == 0 {
            return false;
        }
        if freshness == ReadFreshness::FreshRequired {
            return true;
        }
        let now_ms = Self::current_epoch_ms();
        let last_refresh_started_ms = self
            .reader_state
            .last_refresh_started_ms
            .load(Ordering::Acquire);
        let last_refresh_completed_ms = self
            .reader_state
            .last_refresh_completed_ms
            .load(Ordering::Acquire);
        let last_refresh_ms = last_refresh_started_ms.max(last_refresh_completed_ms);
        let refresh_age_ms = now_ms.saturating_sub(last_refresh_ms);
        lag > Self::reader_refresh_small_lag_epochs()
            || refresh_age_ms >= Self::reader_refresh_request_debounce_ms()
    }

    pub(crate) fn current_epoch_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn record_reader_read(&self) {
        self.reader_state
            .reads_on_reader_total
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_writer_read(&self, freshness: ReadFreshness) {
        self.reader_state
            .reads_on_writer_total
            .fetch_add(1, Ordering::Relaxed);
        if freshness == ReadFreshness::FreshRequired {
            self.reader_state
                .fresh_required_fallback_writer_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn query_targets_attached_soll(query: &str) -> bool {
        query.to_ascii_lowercase().contains("soll.")
    }

    fn query_json_on_writer(&self, query: &str) -> Result<String> {
        // REQ-AXO-254: under PG, the rewriter must run on writer-routed
        // reads too — `query_on_ctx` is the raw FFI call. Without this
        // path, queries that hit the writer (read-only SQL gateway,
        // SOLL-targeted reads, OptimizerDecisionLog probes, etc.) skip
        // the DuckDB→PG translations and emit unqualified table names
        // that PG rejects.
        let normalized = self.normalize_attached_soll_query(query);
        let writer = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(normalized.as_ref(), *writer)
    }

    fn query_count_on_writer(&self, query: &str) -> Result<i64> {
        let normalized = self.normalize_attached_soll_query(query);
        let writer = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe {
            let count_fn = self.pool.symbols.query_count_fn;
            Ok(count_fn(
                *writer,
                CString::new(normalized.as_ref())?.as_ptr(),
            ))
        }
    }

    fn select_read_route(&self, query: &str, freshness: ReadFreshness) -> ReadRoute {
        if Self::query_targets_attached_soll(query) {
            self.record_writer_read(freshness);
            return ReadRoute::Writer;
        }

        if self.db_path.is_none() {
            self.record_writer_read(freshness);
            return ReadRoute::Writer;
        }

        if self.reader_only_ist_mode {
            self.record_reader_read();
            return ReadRoute::Reader;
        }

        let commit_epoch = self.reader_state.commit_epoch.load(Ordering::Acquire);
        let reader_epoch = self.reader_state.reader_epoch.load(Ordering::Acquire);
        let lag = commit_epoch.saturating_sub(reader_epoch);
        let reader_available = self.reader_snapshot_reader_available();
        let ist_snapshot_contract = self.reader_snapshot_freshness_contract();

        if !reader_available || !matches!(ist_snapshot_contract.state, RuntimeFreshnessState::Fresh)
        {
            self.request_reader_refresh_up_to(commit_epoch.max(1));
            self.record_writer_read(freshness);
            return ReadRoute::Writer;
        }

        if lag == 0 {
            self.record_reader_read();
            return ReadRoute::Reader;
        }

        if self.should_request_reader_refresh_for_read(freshness, lag) {
            self.request_reader_refresh_up_to(commit_epoch);
        }
        match freshness {
            ReadFreshness::StaleOk => {
                self.record_reader_read();
                ReadRoute::Reader
            }
            ReadFreshness::FreshPreferred => {
                self.record_reader_read();
                ReadRoute::Reader
            }
            ReadFreshness::FreshRequired => {
                self.record_writer_read(freshness);
                ReadRoute::Writer
            }
        }
    }

    pub(crate) fn query_json_on_reader_with_freshness(
        &self,
        query: &str,
        freshness: ReadFreshness,
    ) -> Result<String> {
        // REQ-AXO-254: rewriter must run on the reader-routed read too.
        // `query_on_ctx` (line 348) calls the raw FFI directly, so the
        // PG translations must happen before we hand off the SQL string.
        let normalized = self.normalize_attached_soll_query(query);
        match self.select_read_route(query, freshness) {
            ReadRoute::Writer => self.query_json_on_writer(normalized.as_ref()),
            ReadRoute::Reader => {
                let guard = self
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if (*guard).is_null() {
                    drop(guard);
                    if self.reader_only_ist_mode {
                        return Err(self.reader_only_ist_unavailable_error());
                    }
                    self.record_writer_read(freshness);
                    return self.query_json_on_writer(normalized.as_ref());
                }
                let result = self.query_on_ctx(normalized.as_ref(), *guard);
                drop(guard);
                result
            }
        }
    }

    pub(crate) fn query_json_on_reader(&self, query: &str) -> Result<String> {
        self.query_json_on_reader_with_freshness(query, ReadFreshness::FreshPreferred)
    }

    pub(crate) fn query_count_on_reader_with_freshness(
        &self,
        query: &str,
        freshness: ReadFreshness,
    ) -> Result<i64> {
        let normalized = self.normalize_attached_soll_query(query);
        match self.select_read_route(query, freshness) {
            ReadRoute::Writer => self.query_count_on_writer(normalized.as_ref()),
            ReadRoute::Reader => {
                let guard = self
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if (*guard).is_null() {
                    drop(guard);
                    if self.reader_only_ist_mode {
                        return Err(self.reader_only_ist_unavailable_error());
                    }
                    self.record_writer_read(freshness);
                    let writer = self
                        .pool
                        .writer_ctx
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    return unsafe {
                        let count_fn = self.pool.symbols.query_count_fn;
                        Ok(count_fn(
                            *writer,
                            CString::new(normalized.as_ref())?.as_ptr(),
                        ))
                    };
                }
                unsafe {
                    let count_fn = self.pool.symbols.query_count_fn;
                    let result = Ok(count_fn(
                        *guard,
                        CString::new(normalized.as_ref())?.as_ptr(),
                    ));
                    drop(guard);
                    result
                }
            }
        }
    }

    pub(crate) fn query_count_on_reader(&self, query: &str) -> Result<i64> {
        self.query_count_on_reader_with_freshness(query, ReadFreshness::FreshPreferred)
    }

    pub fn execute_raw_sql_gateway(&self, query: &str) -> Result<String> {
        if is_read_only_sql(query) {
            if self.reader_only_ist_mode && !Self::query_targets_attached_soll(query) {
                return self.query_json_on_reader_with_freshness(query, ReadFreshness::StaleOk);
            }
            // SQL gateway is the dashboard's canonical truth surface.
            // Force read-only SQL through writer ctx to avoid reader/writer snapshot oscillation.
            self.record_writer_read(ReadFreshness::FreshRequired);
            return self.query_json_on_writer(query);
        }

        self.execute(query)?;
        Ok("{\"ok\":true}".to_string())
    }

    fn graph_projection_version() -> &'static str {
        "1"
    }

    fn projection_signature(entries: &[String]) -> String {
        let mut normalized = entries.to_vec();
        normalized.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        normalized.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn graph_projection_state_matches(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
        signature: &str,
        version: &str,
    ) -> Result<bool> {
        let res = self.query_json_param_with_freshness(
            "SELECT source_signature, projection_version \
             FROM GraphProjectionState \
             WHERE anchor_type = $anchor_type \
               AND anchor_id = $anchor_id \
               AND radius = $radius \
             LIMIT 1",
            &serde_json::json!({
                "anchor_type": anchor_type,
                "anchor_id": anchor_id,
                "radius": radius,
            }),
            ReadFreshness::FreshRequired,
        )?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(false);
        };
        let Some(existing_signature) = row.first().and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        let Some(existing_version) = row.get(1).and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        Ok(existing_signature == signature && existing_version == version)
    }

    fn resolve_symbol_anchor_id(&self, symbol: &str) -> Result<Option<String>> {
        let res = self.query_json_param_with_freshness(
            "SELECT id FROM Symbol WHERE id = $sym OR name = $sym LIMIT 1",
            &serde_json::json!({ "sym": symbol }),
            ReadFreshness::FreshRequired,
        )?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()))
    }

    pub fn refresh_symbol_projection(&self, symbol: &str, radius: u64) -> Result<Option<String>> {
        let Some(anchor_id) = self.resolve_symbol_anchor_id(symbol)? else {
            return Ok(None);
        };

        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS / CALLS_NIF
        // tables are empty/dropped — the GraphProjection cache cannot be
        // refreshed via SQL. Skip the refresh; downstream consumers either
        // (a) still see the previously-cached projection or (b) get an empty
        // projection (acceptable because authoritative call-graph reads now
        // go through AGE primary tools). An AGE Cypher equivalent for the
        // projection refresh is tracked separately on REQ-AXO-251.
        if self.skip_sql_relations() {
            return Ok(Some(anchor_id));
        }

        let radius = radius.max(1) as i64;
        let params = serde_json::json!({
            "anchor": anchor_id,
            "radius": radius,
        });
        let query = "WITH RECURSIVE \
                call_edges(source_id, target_id) AS ( \
                    SELECT source_id, target_id FROM CALLS \
                    UNION ALL \
                    SELECT source_id, target_id FROM CALLS_NIF \
                    UNION ALL \
                    SELECT target_id, source_id FROM CALLS \
                    UNION ALL \
                    SELECT target_id, source_id FROM CALLS_NIF \
                ), \
                traverse(node_id, distance) AS ( \
                    SELECT $anchor AS node_id, 0 AS distance \
                    UNION ALL \
                    SELECT e.target_id, t.distance + 1 \
                    FROM call_edges e JOIN traverse t ON e.source_id = t.node_id \
                    WHERE t.distance < $radius \
                ) \
            SELECT node_id, MIN(distance) \
            FROM traverse \
            GROUP BY node_id";
        let res =
            self.query_json_param_with_freshness(query, &params, ReadFreshness::FreshRequired)?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let created_at = chrono::Utc::now().timestamp_millis();
        let anchor_escaped = anchor_id.replace('\'', "''");
        let version = Self::graph_projection_version();
        let mut signature_entries = vec![format!(
            "symbol|{}|symbol|{}|anchor|0",
            anchor_id, anchor_id
        )];

        for row in &rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(0);
            if node_id == anchor_id {
                continue;
            }
            signature_entries.push(format!(
                "symbol|{}|symbol|{}|call-neighborhood|{}",
                anchor_id, node_id, distance
            ));
        }
        let signature = Self::projection_signature(&signature_entries);

        if self.graph_projection_state_matches("symbol", &anchor_id, radius, &signature, version)? {
            return Ok(Some(anchor_id));
        }

        let mut queries = vec![format!(
            "DELETE FROM GraphProjection WHERE anchor_type = 'symbol' AND anchor_id = '{}' AND radius = {};",
            anchor_escaped, radius
        )];
        queries.push(format!(
            "DELETE FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = '{}' AND radius = {};",
            anchor_escaped, radius
        ));

        queries.push(format!(
            "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('symbol', '{}', 'symbol', '{}', 'anchor', 0, {}, '{}', {});",
            anchor_escaped, anchor_escaped, radius, version, created_at
        ));

        for row in rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(0);
            if node_id == anchor_id {
                continue;
            }
            queries.push(format!(
                "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('symbol', '{}', 'symbol', '{}', 'call-neighborhood', {}, {}, '{}', {});",
                anchor_escaped,
                node_id.replace('\'', "''"),
                distance,
                radius,
                version,
                created_at
            ));
        }
        queries.push(format!(
            "INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', '{}', {}, '{}', '{}', {});",
            anchor_escaped, radius, signature, version, created_at
        ));

        self.execute_batch(&queries)?;
        Ok(Some(anchor_id))
    }

    pub fn refresh_file_projection(&self, file_path: &str, radius: u64) -> Result<()> {
        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS / CONTAINS
        // tables are empty/dropped — skip the projection refresh (mirrors
        // refresh_symbol_projection). Authoritative file-call traversal goes
        // through AGE primary tools (axon_path / axon_impact).
        if self.skip_sql_relations() {
            return Ok(());
        }
        let radius = radius.max(1) as i64;
        let params = serde_json::json!({
            "file": file_path,
            "radius": radius,
        });
        let query = "WITH RECURSIVE \
                call_edges(source_id, target_id) AS ( \
                    SELECT source_id, target_id FROM CALLS \
                    UNION ALL \
                    SELECT target_id, source_id FROM CALLS \
                ), \
                seed(node_id, distance) AS ( \
                    SELECT target_id, 1 AS distance FROM CONTAINS WHERE source_id = $file \
                    UNION ALL \
                    SELECT e.target_id, s.distance + 1 \
                    FROM call_edges e JOIN seed s ON e.source_id = s.node_id \
                    WHERE s.distance < $radius \
                ) \
            SELECT node_id, MIN(distance) \
            FROM seed \
            GROUP BY node_id";
        let res =
            self.query_json_param_with_freshness(query, &params, ReadFreshness::FreshRequired)?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let created_at = chrono::Utc::now().timestamp_millis();
        let file_escaped = file_path.replace('\'', "''");
        let version = Self::graph_projection_version();
        let mut signature_entries = vec![format!("file|{}|file|{}|file|0", file_path, file_path)];

        for row in &rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(1);
            let edge_kind = if distance == 1 {
                "contains"
            } else {
                "call-neighborhood"
            };
            signature_entries.push(format!(
                "file|{}|symbol|{}|{}|{}",
                file_path, node_id, edge_kind, distance
            ));
        }
        let signature = Self::projection_signature(&signature_entries);

        if self.graph_projection_state_matches("file", file_path, radius, &signature, version)? {
            return Ok(());
        }

        let mut queries = vec![format!(
            "DELETE FROM GraphProjection WHERE anchor_type = 'file' AND anchor_id = '{}' AND radius = {};",
            file_escaped, radius
        )];
        queries.push(format!(
            "DELETE FROM GraphProjectionState WHERE anchor_type = 'file' AND anchor_id = '{}' AND radius = {};",
            file_escaped, radius
        ));

        queries.push(format!(
            "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('file', '{}', 'file', '{}', 'file', 0, {}, '{}', {});",
            file_escaped, file_escaped, radius, version, created_at
        ));

        for row in rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(1);
            let edge_kind = if distance == 1 {
                "contains"
            } else {
                "call-neighborhood"
            };
            queries.push(format!(
                "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('file', '{}', 'symbol', '{}', '{}', {}, {}, '{}', {});",
                file_escaped,
                node_id.replace('\'', "''"),
                edge_kind,
                distance,
                radius,
                version,
                created_at
            ));
        }
        queries.push(format!(
            "INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('file', '{}', {}, '{}', '{}', {});",
            file_escaped, radius, signature, version, created_at
        ));

        self.execute_batch(&queries)
    }

    pub fn query_graph_projection(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: u64,
    ) -> Result<String> {
        let query = "SELECT gp.target_type, gp.target_id, gp.edge_kind, gp.distance, \
                            COALESCE(s.name, gp.target_id) AS label, \
                            COALESCE(f.path, contain.source_id, '') AS uri \
                     FROM GraphProjection gp \
                     LEFT JOIN Symbol s ON gp.target_type = 'symbol' AND s.id = gp.target_id \
                     LEFT JOIN CONTAINS contain ON gp.target_type = 'symbol' AND contain.target_id = gp.target_id \
                     LEFT JOIN File f ON gp.target_type = 'file' AND f.path = gp.target_id \
                     WHERE gp.anchor_type = $anchor_type AND gp.anchor_id = $anchor_id AND gp.radius = $radius \
                     ORDER BY gp.distance ASC, gp.edge_kind ASC, label ASC";
        self.query_json_param(
            query,
            &serde_json::json!({
                "anchor_type": anchor_type,
                "anchor_id": anchor_id,
                "radius": radius as i64,
            }),
        )
    }

    pub fn execute(&self, query: &str) -> Result<()> {
        let normalized = self.normalize_attached_soll_query(query);
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe {
            let exec_fn = self.pool.symbols.exec_fn;
            if !exec_fn(*guard, CString::new(normalized.as_ref())?.as_ptr()) {
                return Err(anyhow!("Writer Error: {}", normalized.as_ref()));
            }
        }
        if !self.reader_only_ist_mode {
            self.mark_writer_commit_visible();
        }
        Ok(())
    }

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<()> {
        let expanded = Self::expand_named_params(query, params)?;
        self.execute(&expanded)
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        self.query_json_on_reader(query)
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        let expanded = Self::expand_named_params(query, params)?;
        self.query_json_on_reader(&expanded)
    }

    pub(crate) fn query_json_param_with_freshness(
        &self,
        query: &str,
        params: &serde_json::Value,
        freshness: ReadFreshness,
    ) -> Result<String> {
        let expanded = Self::expand_named_params(query, params)?;
        self.query_json_on_reader_with_freshness(&expanded, freshness)
    }

    pub fn query_json_writer(&self, query: &str) -> Result<String> {
        self.query_json_on_writer(query)
    }

    pub(crate) fn query_count_writer(&self, query: &str) -> Result<i64> {
        self.query_count_on_writer(query)
    }

    pub(crate) fn query_single_i64_writer(&self, query: &str) -> Result<Option<i64>> {
        let raw = self.query_json_on_writer(query)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let Some(val) = row.first() else {
            return Ok(None);
        };
        if let Some(number) = val.as_i64() {
            return Ok(Some(number));
        }
        if let Some(text) = val.as_str() {
            return Ok(text.parse::<i64>().ok());
        }
        Ok(None)
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        self.query_count_on_reader(query)
    }

    pub fn query_count_param(&self, query: &str, params: &serde_json::Value) -> Result<i64> {
        let res = self.query_json_param(query, params)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res).unwrap_or_default();
        if let Some(row) = rows.first() {
            if let Some(val) = row.first() {
                if let Some(number) = val.as_i64() {
                    return Ok(number);
                }
                if let Some(text) = val.as_str() {
                    return Ok(text.parse::<i64>().unwrap_or(0));
                }
            }
        }
        Ok(0)
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<()> {
        if queries.is_empty() {
            return Ok(());
        }

        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        // REQ-AXO-244 / FFI connection-pinning fix: under the PG plugin
        // each `pg_execute` call gets a fresh connection from the
        // deadpool pool, which breaks the BEGIN/…/COMMIT pairing —
        // BEGIN ends up on connection A, COMMIT on connection D, and
        // connection A stays "idle in transaction" indefinitely
        // holding row locks. The fix joins the entire batch into a
        // single `BEGIN; <q1>; <q2>; …; COMMIT;` string and dispatches
        // it via one `pg_execute` call so the whole sequence runs on
        // one pinned connection. tokio_postgres' `batch_execute` is
        // happy with a multi-statement string and DuckDB's plugin
        // batch_execute behaves identically — same shape on both
        // backends, so the join is unconditional.
        let mut combined = String::with_capacity(
            queries.iter().map(|q| q.len() + 2).sum::<usize>() + 32,
        );
        combined.push_str("BEGIN;\n");
        for q in queries {
            let normalized = self.normalize_attached_soll_query(q);
            combined.push_str(normalized.as_ref());
            // Many of our queries already end with `;`. Add a guard
            // separator if not — the parser is forgiving with extra
            // semicolons but rejects two adjacent statements without
            // one.
            if !normalized.as_ref().trim_end().ends_with(';') {
                combined.push(';');
            }
            combined.push('\n');
        }
        combined.push_str("COMMIT;");


        unsafe {
            let exec_fn = self.pool.symbols.exec_fn;
            let c_query = match CString::new(combined) {
                Ok(c) => c,
                Err(e) => {
                    return Err(anyhow!("Batch Writer Error (CString): {:?}", e));
                }
            };
            if !exec_fn(*guard, c_query.as_ptr()) {
                // Note: we don't issue an explicit ROLLBACK here. The
                // joined batch failed inside the pg_execute call, and
                // `batch_execute` already aborts the transaction on
                // any statement error. No further state cleanup needed
                // — the connection returns to the pool with the
                // implicit rollback already in effect (and DuckDB's
                // batch_execute behaves the same way).
                return Err(anyhow!(
                    "Batch Writer Error on batch (size={})",
                    queries.len()
                ));
            }
        }
        if !self.reader_only_ist_mode {
            self.mark_writer_commit_visible();
        }
        Ok(())
    }

    pub(crate) fn query_on_ctx(&self, query: &str, ctx: *mut std::ffi::c_void) -> Result<String> {
        let normalized = self.normalize_attached_soll_query(query);
        unsafe {
            let query_fn = self.pool.symbols.query_json_fn;
            let free_fn = self.pool.symbols.free_str_fn;
            let ptr = query_fn(ctx, CString::new(normalized.as_ref())?.as_ptr());
            if ptr.is_null() {
                return Ok("[]".to_string());
            }
            let res = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
            free_fn(ptr);
            // REQ-AXO-129 — detect plugin error envelope. Legitimate
            // results are always a JSON array (`[`); error envelopes
            // are always a JSON object (`{`) carrying
            // `_axon_plugin_error`. This unwraps the silent-[] trap:
            // column-not-found / table-not-found / Prepare errors
            // now surface as `Err` instead of an indistinguishable
            // empty result.
            if res.starts_with('{') {
                if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&res) {
                    if let Some(message) = envelope
                        .get("_axon_plugin_error")
                        .and_then(|v| v.as_str())
                    {
                        return Err(anyhow::anyhow!(
                            "Graph plugin error: {message}"
                        ));
                    }
                }
            }
            Ok(res)
        }
    }

    fn expand_named_params(query: &str, params: &serde_json::Value) -> Result<String> {
        if let Some(arr) = params.as_array() {
            // REQ-AXO-091 — single-pass scan that consumes one positional
            // parameter per `?` in the original query. The previous
            // implementation used `expanded.find('?')` after each
            // substitution, which matched literal `?` chars that landed
            // inside an already-substituted user string (e.g. a title
            // like "does this fail?"). That produced malformed SQL
            // because the next param overwrote the user's `?` instead
            // of the next placeholder. Tracking quote context skips
            // `?` chars inside SQL string literals as well.
            let mut iter = arr.iter();
            let mut result = String::with_capacity(query.len() + arr.len() * 16);
            let mut in_single_quote = false;
            let mut chars = query.chars().peekable();
            while let Some(ch) = chars.next() {
                match ch {
                    '\'' => {
                        if in_single_quote && chars.peek() == Some(&'\'') {
                            // Escaped quote inside a string literal ('') — emit both chars.
                            result.push('\'');
                            result.push('\'');
                            chars.next();
                        } else {
                            in_single_quote = !in_single_quote;
                            result.push('\'');
                        }
                    }
                    '?' if !in_single_quote => {
                        let value = iter.next().ok_or_else(|| {
                            anyhow!("Too few positional parameters supplied")
                        })?;
                        let replacement = match value {
                            serde_json::Value::Null => "NULL".to_string(),
                            serde_json::Value::Bool(v) => v.to_string(),
                            serde_json::Value::Number(v) => v.to_string(),
                            serde_json::Value::String(v) => {
                                format!("'{}'", v.replace('\'', "''"))
                            }
                            _ => {
                                return Err(anyhow!(
                                    "Unsupported positional parameter type: {}",
                                    value
                                ))
                            }
                        };
                        result.push_str(&replacement);
                    }
                    _ => result.push(ch),
                }
            }
            if iter.next().is_some() {
                return Err(anyhow!("Too many positional parameters supplied"));
            }
            return Ok(result);
        }

        let mut expanded = query.to_string();
        let obj = match params.as_object() {
            Some(obj) => obj,
            None => return Ok(expanded),
        };

        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::Null => "NULL".to_string(),
                serde_json::Value::Bool(v) => v.to_string(),
                serde_json::Value::Number(v) => v.to_string(),
                serde_json::Value::String(v) => format!("'{}'", v.replace('\'', "''")),
                _ => {
                    return Err(anyhow!(
                        "Unsupported parameter type for ${}: {}",
                        key,
                        value
                    ))
                }
            };
            expanded = expanded.replace(&format!("${}", key), &replacement);
        }

        Ok(expanded)
    }
}

fn is_read_only_sql(query: &str) -> bool {
    let trimmed = query.trim_start();
    let lowered = trimmed.to_ascii_lowercase();
    matches!(
        lowered.split_whitespace().next(),
        Some("select" | "with" | "pragma" | "show" | "describe" | "explain")
    )
}

// REQ-AXO-091 placeholder-expansion tests live in a sibling file so the
// commit's diff path satisfies the TDD guideline (GUI-PRO-001) which
// expects a `_tests.rs` companion path.
#[cfg(test)]
#[path = "graph_query_tests.rs"]
mod expand_params_tests;

#[cfg(test)]
mod tests {
    use super::ReadFreshness;
    use crate::graph::GraphStore;
    use std::ffi::CString;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;

    fn create_test_db_with_distinct_reader() -> (tempfile::TempDir, GraphStore) {
        let tempdir = tempdir().unwrap();
        let store = GraphStore::new(tempdir.path().to_str().unwrap()).unwrap();
        attach_distinct_reader_snapshot(&store);
        (tempdir, store)
    }

    fn attach_distinct_reader_snapshot(store: &GraphStore) {
        let db_path = store
            .db_path
            .as_ref()
            .expect("disk-backed test store required for distinct reader");
        let reader_c_path = CString::new(db_path.to_string_lossy().to_string()).unwrap();
        let soll_path = {
            let mut path = PathBuf::from(db_path);
            path.set_file_name("soll.db");
            path
        };
        let attach_q = format!(
            "INSTALL json; LOAD json; SET checkpoint_threshold = '1GB'; ATTACH '{}' AS soll;",
            soll_path.to_string_lossy().replace("'", "''")
        );

        unsafe {
            let init_fn = store.pool.symbols.init_fn;
            let exec_fn = store.pool.symbols.exec_fn;
            let reader_ptr = init_fn(reader_c_path.as_ptr(), true);
            assert!(
                !reader_ptr.is_null(),
                "failed to initialize distinct reader"
            );
            assert!(exec_fn(
                reader_ptr,
                CString::new(attach_q).unwrap().as_ptr()
            ));

            let mut reader_guard = store
                .pool
                .reader_ctx
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *reader_guard = reader_ptr;
        }
        store.refresh_reader_snapshot().unwrap();
    }

    #[test]
    fn normalize_attached_soll_query_qualifies_legacy_soll_paths() {
        let tempdir = tempdir().unwrap();
        let store = GraphStore::new(tempdir.path().to_str().unwrap()).unwrap();

        let normalized = store
            .normalize_attached_soll_query("SELECT * FROM soll.Registry WHERE id = 'AXON_GLOBAL'");

        assert_eq!(
            normalized.as_ref(),
            "SELECT * FROM soll.main.Registry WHERE id = 'AXON_GLOBAL'"
        );
    }

    #[test]
    fn rewrite_duckdb_json_helpers_collapses_cast_varchar_pattern() {
        // REQ-AXO-249 — completeness_coverage.rs:174 emits this exact
        // shape to filter requirements without acceptance_criteria.
        let input = "AND COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), '') IN ('', '[]')";
        let expected = "AND COALESCE((r.metadata->>'acceptance_criteria'), '') IN ('', '[]')";
        assert_eq!(
            super::rewrite_duckdb_json_helpers_for_pg(input).as_ref(),
            expected
        );
    }

    #[test]
    fn rewrite_duckdb_json_helpers_translates_extract_string() {
        // REQ-AXO-249 — workflow_project.rs:489 / 531 priority + updated_at.
        let input = "SELECT id, COALESCE(json_extract_string(metadata, '$.priority'), '') FROM soll.Node";
        let expected = "SELECT id, COALESCE((metadata->>'priority'), '') FROM soll.Node";
        assert_eq!(
            super::rewrite_duckdb_json_helpers_for_pg(input).as_ref(),
            expected
        );
    }

    #[test]
    fn rewrite_duckdb_json_helpers_passthrough_when_no_match() {
        let input = "SELECT 1";
        let out = super::rewrite_duckdb_json_helpers_for_pg(input);
        assert_eq!(out.as_ref(), input);
        // Cow::Borrowed when no rewrite needed (no allocation cost on
        // the common path).
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn rewrite_duckdb_json_helpers_handles_qualified_column_reference() {
        // r.metadata vs metadata — both must work.
        let input = "json_extract_string(r.metadata, '$.tag')";
        let expected = "(r.metadata->>'tag')";
        assert_eq!(
            super::rewrite_duckdb_json_helpers_for_pg(input).as_ref(),
            expected
        );
    }

    #[test]
    fn rewrite_duckdb_collapses_3part_soll_main_to_2part() {
        // REQ-AXO-254 — DuckDB-only `soll.main.X` 3-part identifiers must
        // collapse to `soll.X` for PostgreSQL.
        let input = "SELECT description FROM soll.main.Node WHERE id = 'DEC-PRO-001'";
        let expected = "SELECT description FROM soll.Node WHERE id = 'DEC-PRO-001'";
        assert_eq!(
            super::rewrite_duckdb_json_helpers_for_pg(input).as_ref(),
            expected
        );
    }

    #[test]
    fn rewrite_duckdb_qualifies_optimizerdecisionlog_with_axon_runtime_schema() {
        // REQ-AXO-254 — bare `OptimizerDecisionLog` must be qualified with
        // `axon_runtime.` under PG (postgres/ddl.rs:204).
        let input = "SELECT decision_id FROM OptimizerDecisionLog ORDER BY at_ms DESC LIMIT 1";
        let expected =
            "SELECT decision_id FROM axon_runtime.OptimizerDecisionLog ORDER BY at_ms DESC LIMIT 1";
        assert_eq!(
            super::rewrite_duckdb_json_helpers_for_pg(input).as_ref(),
            expected
        );
    }

    #[test]
    fn rewrite_duckdb_optimizerdecisionlog_idempotent_when_already_qualified() {
        let input = "INSERT INTO axon_runtime.OptimizerDecisionLog (decision_id) VALUES ('x')";
        let out = super::rewrite_duckdb_json_helpers_for_pg(input);
        assert_eq!(out.as_ref(), input);
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn execute_raw_sql_gateway_supports_read_only_and_mutating_queries() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();

        let read = store.execute_raw_sql_gateway("SELECT 1").unwrap();
        assert!(read.contains("1"), "{read}");

        let write = store
            .execute_raw_sql_gateway(
                "INSERT INTO File (path, project_code) VALUES ('/tmp/sql_gateway.ex', 'PRJ')",
            )
            .unwrap();
        assert!(write.contains("\"ok\":true"), "{write}");

        let count = store
            .query_count("SELECT count(*) FROM File WHERE path = '/tmp/sql_gateway.ex'")
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn stale_ok_reader_requests_refresh_without_writer_fallback() {
        let (_tempdir, store) = create_test_db_with_distinct_reader();
        let before = store.reader_snapshot_diagnostics();
        store.reader_state.commit_epoch.store(7, Ordering::Relaxed);
        store.reader_state.reader_epoch.store(5, Ordering::Relaxed);
        store
            .reader_state
            .refresh_requested_epoch
            .store(0, Ordering::Relaxed);
        store
            .reader_state
            .refresh_inflight
            .store(false, Ordering::Relaxed);
        store
            .reader_state
            .last_refresh_started_ms
            .store(0, Ordering::Relaxed);
        store
            .reader_state
            .last_refresh_completed_ms
            .store(0, Ordering::Relaxed);

        let _ = store
            .query_json_on_reader_with_freshness("SELECT 1", ReadFreshness::StaleOk)
            .unwrap();

        let snapshot = store.reader_snapshot_diagnostics();
        assert_eq!(
            snapshot.reads_on_reader_total - before.reads_on_reader_total,
            1
        );
        assert_eq!(
            snapshot.reads_on_writer_total - before.reads_on_writer_total,
            0
        );
        assert!(snapshot.refresh_inflight);
        assert_eq!(snapshot.refresh_requested_epoch, 7);
    }

    #[test]
    fn fresh_required_routes_stale_reads_to_writer() {
        let (_tempdir, store) = create_test_db_with_distinct_reader();
        let before = store.reader_snapshot_diagnostics();
        store.reader_state.commit_epoch.store(9, Ordering::Relaxed);
        store.reader_state.reader_epoch.store(3, Ordering::Relaxed);
        store
            .reader_state
            .refresh_requested_epoch
            .store(0, Ordering::Relaxed);
        store
            .reader_state
            .refresh_inflight
            .store(false, Ordering::Relaxed);

        let _ = store
            .query_json_on_reader_with_freshness("SELECT 1", ReadFreshness::FreshRequired)
            .unwrap();

        let snapshot = store.reader_snapshot_diagnostics();
        assert_eq!(
            snapshot.reads_on_reader_total - before.reads_on_reader_total,
            0
        );
        assert_eq!(
            snapshot.reads_on_writer_total - before.reads_on_writer_total,
            1
        );
        assert_eq!(
            snapshot.fresh_required_fallback_writer_total
                - before.fresh_required_fallback_writer_total,
            1
        );
        assert!(snapshot.refresh_inflight);
        assert_eq!(snapshot.refresh_requested_epoch, 9);
    }

    #[test]
    fn fresh_preferred_stays_on_reader_and_requests_refresh() {
        let (_tempdir, store) = create_test_db_with_distinct_reader();
        let before = store.reader_snapshot_diagnostics();
        store.reader_state.commit_epoch.store(15, Ordering::Relaxed);
        store.reader_state.reader_epoch.store(3, Ordering::Relaxed);
        store
            .reader_state
            .refresh_requested_epoch
            .store(0, Ordering::Relaxed);
        store
            .reader_state
            .refresh_inflight
            .store(false, Ordering::Relaxed);
        store.recent_write_epoch_ms.store(0, Ordering::Relaxed);
        store
            .reader_state
            .last_refresh_started_ms
            .store(0, Ordering::Relaxed);
        store
            .reader_state
            .last_refresh_completed_ms
            .store(0, Ordering::Relaxed);

        let _ = store
            .query_json_on_reader_with_freshness("SELECT 1", ReadFreshness::FreshPreferred)
            .unwrap();

        let snapshot = store.reader_snapshot_diagnostics();
        assert_eq!(
            snapshot.reads_on_reader_total - before.reads_on_reader_total,
            1
        );
        assert_eq!(
            snapshot.reads_on_writer_total - before.reads_on_writer_total,
            0
        );
        assert_eq!(snapshot.refresh_requested_epoch, 15);
    }

    #[test]
    fn fresh_preferred_small_recent_lag_does_not_request_refresh() {
        let (_tempdir, store) = create_test_db_with_distinct_reader();
        let now_ms = crate::graph::GraphStore::current_epoch_ms();
        let before = store.reader_snapshot_diagnostics();
        store.reader_state.commit_epoch.store(15, Ordering::Relaxed);
        store.reader_state.reader_epoch.store(14, Ordering::Relaxed);
        store
            .reader_state
            .refresh_requested_epoch
            .store(14, Ordering::Relaxed);
        store
            .reader_state
            .refresh_inflight
            .store(false, Ordering::Relaxed);
        store
            .reader_state
            .last_refresh_started_ms
            .store(now_ms, Ordering::Relaxed);
        store
            .reader_state
            .last_refresh_completed_ms
            .store(now_ms, Ordering::Relaxed);

        let _ = store
            .query_json_on_reader_with_freshness("SELECT 1", ReadFreshness::FreshPreferred)
            .unwrap();

        let snapshot = store.reader_snapshot_diagnostics();
        assert_eq!(
            snapshot.reads_on_reader_total - before.reads_on_reader_total,
            1
        );
        assert_eq!(
            snapshot.reads_on_writer_total - before.reads_on_writer_total,
            0
        );
        assert!(!snapshot.refresh_inflight);
        assert_eq!(snapshot.refresh_requested_epoch, 14);
    }

    #[test]
    fn reader_refresh_syncs_epoch_to_commit() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.reader_state.commit_epoch.store(12, Ordering::Relaxed);
        store.reader_state.reader_epoch.store(4, Ordering::Relaxed);

        store.refresh_reader_snapshot().unwrap();

        let snapshot = store.reader_snapshot_diagnostics();
        assert_eq!(snapshot.commit_epoch, 12);
        assert_eq!(snapshot.reader_epoch, 12);
        assert_eq!(snapshot.reader_epoch_lag, 0);
        assert!(!snapshot.refresh_inflight);
    }

    #[test]
    fn reader_only_mode_never_falls_back_to_writer_when_reader_is_unavailable() {
        let tempdir = tempdir().unwrap();
        let db_root = tempdir.path().to_str().unwrap();
        drop(GraphStore::new(db_root).unwrap());

        let store = GraphStore::new_brain_reader_soll_writer(db_root).unwrap();
        let before = store.reader_snapshot_diagnostics();

        let reader_ptr = {
            let mut reader_guard = store
                .pool
                .reader_ctx
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let ptr = *reader_guard;
            *reader_guard = std::ptr::null_mut();
            ptr
        };
        assert!(!reader_ptr.is_null());

        let err = store
            .query_json_on_reader_with_freshness("SELECT 1", ReadFreshness::StaleOk)
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("IST reader-only access unavailable in split brain mode"));

        let after = store.reader_snapshot_diagnostics();
        assert_eq!(
            after.reads_on_writer_total - before.reads_on_writer_total,
            0
        );
        assert_eq!(
            after.fresh_required_fallback_writer_total
                - before.fresh_required_fallback_writer_total,
            0
        );
    }

    #[test]
    fn in_memory_store_reads_route_to_writer_without_reader_refresh() {
        let store = GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap();
        store.execute("CREATE TABLE Demo (value INTEGER)").unwrap();
        store
            .execute("INSERT INTO Demo (value) VALUES (1)")
            .unwrap();
        let before = store.reader_snapshot_diagnostics();

        let raw = store.query_json("SELECT value FROM Demo").unwrap();

        assert!(raw.contains('1'));
        let after = store.reader_snapshot_diagnostics();
        assert_eq!(
            after.reads_on_writer_total - before.reads_on_writer_total,
            1
        );
        assert_eq!(
            after.reads_on_reader_total - before.reads_on_reader_total,
            0
        );
        assert_eq!(
            after.refresh_requested_epoch - before.refresh_requested_epoch,
            0
        );
    }

    /// REQ-AXO-129 — `query_on_ctx` must convert plugin error
    /// envelopes to `Err`, so callers see a real failure instead of
    /// the historical silent `Ok("[]")`. This guards the wrapper
    /// contract end-to-end: invalid SQL produces an envelope at the
    /// plugin layer (covered by the plugin's own tests) AND the
    /// graph_query wrapper unwraps that envelope into anyhow::Error.
    #[test]
    fn query_on_ctx_returns_err_for_unknown_table_via_envelope() {
        let store = GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap();
        let result = store.query_json("SELECT * FROM definitely_not_a_table_xyz");
        assert!(
            result.is_err(),
            "REQ-AXO-129: invalid SQL must propagate as Err, got Ok({:?})",
            result.ok()
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Graph plugin error"),
            "error must label the source as Graph plugin, got: {msg}"
        );
    }

    #[test]
    fn query_on_ctx_returns_ok_for_genuine_zero_rows() {
        let store = GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap();
        store
            .execute("CREATE TABLE T (id INTEGER)")
            .unwrap();
        // Valid SQL with empty result must remain Ok("[]") — REQ-AXO-129
        // distinguishes "binder error" from "zero rows" rigorously.
        let result = store
            .query_json("SELECT id FROM T WHERE id = -1")
            .expect("zero-row query must return Ok, not Err");
        assert_eq!(result.trim(), "[]");
    }
}
