use std::borrow::Cow;
use std::ffi::CString;
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
    /// Post-MIL-AXO-017 identity passthrough. The historical DuckDB->PG
    /// compat rewriter (`rewrite_duckdb_json_helpers_for_pg`) was removed:
    /// all SQL sources now emit PG-native syntax directly. This method is
    /// kept as a seam so call-sites don't need a mechanical rename.
    fn normalize_attached_soll_query<'a>(&self, query: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(query)
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

    pub fn refresh_symbol_projection(
        &self,
        symbol: &str,
        _radius: u64,
    ) -> Result<Option<String>> {
        // REQ-AXO-271 slice 2 (post-MIL-AXO-017 / DEC-AXO-083 AGE retirement) :
        // the legacy GraphProjection cache refresh via SQL CALLS / CALLS_NIF
        // tables was conditional on `skip_legacy_relations()`, which always
        // returns true under PG canonical. Authoritative call-graph reads
        // now route through `public.Edge` + db/ddl/04_graph_functions.sql
        // (`callers_of`, `path`, etc.). This function is reduced to anchor
        // resolution: callers receive the resolved id for downstream
        // bookkeeping but no row is written into GraphProjection.
        self.resolve_symbol_anchor_id(symbol)
    }

    pub fn refresh_file_projection(&self, _file_path: &str, _radius: u64) -> Result<()> {
        // REQ-AXO-271 slice 2 : see refresh_symbol_projection rationale.
        // File-call projection refresh via SQL is a no-op under PG canonical.
        Ok(())
    }

    pub fn query_graph_projection(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: u64,
    ) -> Result<String> {
        let query = "SELECT gp.target_type, gp.target_id, gp.edge_kind, gp.distance, \
                            COALESCE(s.name, gp.target_id) AS label, \
                            COALESCE(ch.file_path, '') AS uri \
                     FROM GraphProjection gp \
                     LEFT JOIN Symbol s ON gp.target_type = 'symbol' AND s.id = gp.target_id \
                     LEFT JOIN Chunk ch ON gp.target_type = 'symbol' AND ch.source_id = gp.target_id AND ch.source_type = 'symbol' \
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

    /// REQ-AXO-284 Slice 2 — PG health metrics for the dashboard +
    /// `tools_system_debug` diagnostic surface.
    ///
    /// Returns the canonical database size (`pg_database_size(current_database())`)
    /// in bytes. Errors are absorbed and surfaced as `None` so a transient
    /// catalog hiccup never breaks the telemetry pipeline.
    pub fn pg_database_size_bytes(&self) -> Option<i64> {
        self.query_single_i64_writer("SELECT pg_database_size(current_database())::BIGINT")
            .ok()
            .flatten()
    }

    /// REQ-AXO-284 Slice 2 — size of the per-tenant `ChunkEmbedding` table
    /// (the largest IST table on a populated tenant), including indexes.
    /// Returns `None` when the table is absent (fresh deployment) or on
    /// catalog error.
    pub fn pg_chunkembedding_total_bytes(&self) -> Option<i64> {
        self.query_single_i64_writer(
            "SELECT pg_total_relation_size('public.ChunkEmbedding')::BIGINT",
        )
        .ok()
        .flatten()
    }

    /// REQ-AXO-284 Slice 2 — cumulative WAL volume since the cluster was
    /// last reset (`pg_stat_wal.wal_bytes`, PG 14+). Useful as a
    /// rate-of-change indicator when sampled per heartbeat tick on the
    /// dashboard. Returns `None` if the view is absent (older PG) or on
    /// catalog error.
    pub fn pg_wal_bytes(&self) -> Option<i64> {
        self.query_single_i64_writer("SELECT wal_bytes::BIGINT FROM pg_stat_wal")
            .ok()
            .flatten()
    }

    /// REQ-AXO-284 Slice 2 — PG buffer cache hit ratio for the current
    /// database. Returns ratio in [0.0, 1.0] (multiply by 100 for %).
    /// `None` when `pg_stat_database` has no row yet for current DB (rare,
    /// only on bootstrap) or `blks_hit + blks_read == 0`.
    pub fn pg_buffer_hit_ratio(&self) -> Option<f64> {
        let raw = self.query_json_writer(
            "SELECT blks_hit, blks_read FROM pg_stat_database WHERE datname = current_database()",
        )
        .ok()?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let row = rows.first()?;
        let blks_hit = row.first().and_then(|v| v.as_i64())?;
        let blks_read = row.get(1).and_then(|v| v.as_i64())?;
        let total = blks_hit + blks_read;
        if total <= 0 {
            return None;
        }
        Some(blks_hit as f64 / total as f64)
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
            //
            // Surface the rich `pg_error.message/code/hint` populated by
            // `plugin_db_error_envelope`: the bare `_axon_plugin_error`
            // text is just `tokio_postgres::Error::Display` ("db error")
            // which hides the actual `column "X" does not exist` /
            // SQLSTATE / hint that LLM callers need to self-correct.
            if res.starts_with('{') {
                if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&res) {
                    if let Some(message) = envelope
                        .get("_axon_plugin_error")
                        .and_then(|v| v.as_str())
                    {
                        let pg_msg = envelope
                            .pointer("/pg_error/message")
                            .and_then(|v| v.as_str());
                        let pg_code = envelope
                            .pointer("/pg_error/code")
                            .and_then(|v| v.as_str());
                        let pg_hint = envelope
                            .pointer("/pg_error/hint")
                            .and_then(|v| v.as_str());
                        let mut detail = String::new();
                        if let Some(m) = pg_msg {
                            detail.push_str(m);
                        }
                        if let Some(c) = pg_code {
                            if !detail.is_empty() {
                                detail.push(' ');
                            }
                            detail.push_str(&format!("[SQLSTATE {c}]"));
                        }
                        if let Some(h) = pg_hint {
                            if !detail.is_empty() {
                                detail.push_str(" — ");
                            }
                            detail.push_str(&format!("hint: {h}"));
                        }
                        if detail.is_empty() {
                            return Err(anyhow::anyhow!(
                                "Graph plugin error: {message}"
                            ));
                        }
                        return Err(anyhow::anyhow!(
                            "Graph plugin error: {message} — {detail}"
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
    fn normalize_attached_soll_query_is_identity_passthrough() {
        let tempdir = tempdir().unwrap();
        let store = GraphStore::new(tempdir.path().to_str().unwrap()).unwrap();

        let input = "SELECT * FROM soll.Node WHERE id = 'DEC-PRO-001'";
        let normalized = store.normalize_attached_soll_query(input);
        assert_eq!(normalized.as_ref(), input);
    }

    #[test]
    fn execute_raw_sql_gateway_supports_read_only_and_mutating_queries() {
        // REQ-AXO-901653 slice-5c — File table dropped ; test migrated to
        // public.IndexedFile (3 cols : path, content_hash, last_seen_ms).
        let store = crate::tests::test_helpers::create_test_db().unwrap();

        let read = store.execute_raw_sql_gateway("SELECT 1").unwrap();
        assert!(read.contains("1"), "{read}");

        let write = store
            .execute_raw_sql_gateway(
                "INSERT INTO public.IndexedFile (path, content_hash, last_seen_ms) \
                 VALUES ('/tmp/sql_gateway.ex', 'hash-1', 1)",
            )
            .unwrap();
        assert!(write.contains("\"ok\":true"), "{write}");

        let count = store
            .query_count(
                "SELECT count(*) FROM public.IndexedFile WHERE path = '/tmp/sql_gateway.ex'",
            )
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
        // Valid SQL with empty result must remain Ok("[]") — REQ-AXO-129
        // distinguishes "binder error" from "zero rows" rigorously.
        // Uses `pg_catalog.pg_namespace` (always present, no DDL needed)
        // so the test does not depend on the local user having CREATE
        // privilege on `public`.
        let result = store
            .query_json(
                "SELECT oid FROM pg_catalog.pg_namespace WHERE nspname = 'definitely_not_a_namespace_xyz'",
            )
            .expect("zero-row query must return Ok, not Err");
        assert_eq!(result.trim(), "[]");
    }

    /// The plugin error envelope already carries `pg_error.message/code/hint`
    /// (populated by `plugin_db_error_envelope`). The wrapper must surface
    /// that detail so an LLM sees `column "label" does not exist` instead of
    /// the opaque `db error` from `tokio_postgres::Error::Display`.
    ///
    /// Selects from `pg_catalog.pg_namespace` (always present, never has a
    /// `label` column) so the test does not depend on local DDL fixtures.
    #[test]
    fn query_on_ctx_surfaces_pg_error_detail_for_unknown_column() {
        let store = GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap();
        let result = store.query_json("SELECT label FROM pg_catalog.pg_namespace");
        let err = result.expect_err("unknown column must propagate as Err");
        let msg = err.to_string();
        assert!(
            msg.contains("Graph plugin error"),
            "must keep the Graph plugin prefix, got: {msg}"
        );
        assert!(
            msg.contains("label"),
            "must surface the unknown column name from pg_error.message, got: {msg}"
        );
        assert!(
            msg.contains("SQLSTATE"),
            "must surface the SQLSTATE code, got: {msg}"
        );
    }
}
