use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::graph::GraphStore;

/// REQ-AXO-901884 Stage 0 — typed row decoder for the async-native read path.
/// Each consumer struct impls `from_row` (`row.try_get::<T>(idx)`), and
/// `GraphStore::query_typed` maps rows to it. Replaces, per call-site as it
/// migrates, the `query_json` → `Vec<Vec<String>>` → re-parse contract.
pub trait FromRow: Sized {
    fn from_row(row: &tokio_postgres::Row) -> Result<Self>;
}

impl GraphStore {
    fn query_json_on_writer(&self, query: &str) -> Result<String> {
        // REQ-AXO-901881 W2 — native deadpool read (was the FFI writer ctx +
        // the DuckDB-era `normalize_attached_soll_query` identity seam, #5).
        self.query_native(query)
    }

    fn query_count_on_writer(&self, query: &str) -> Result<i64> {
        Ok(self.pool.native.run_query_count(query))
    }

    pub fn execute_raw_sql_gateway(&self, query: &str) -> Result<String> {
        if is_read_only_sql(query) {
            // SQL gateway is the dashboard's canonical truth surface. Under
            // PG-canonical every read is writer-routed (single pool, MVCC).
            return self.query_json_on_writer(query);
        }

        self.execute(query)?;
        Ok("{\"ok\":true}".to_string())
    }

    fn resolve_symbol_anchor_id(&self, symbol: &str) -> Result<Option<String>> {
        let expanded = Self::expand_named_params(
            "SELECT id FROM Symbol WHERE id = $sym OR name = $sym LIMIT 1",
            &serde_json::json!({ "sym": symbol }),
        )?;
        let res = self.query_json_on_writer(&expanded)?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()))
    }

    pub fn refresh_symbol_projection(&self, symbol: &str, _radius: u64) -> Result<Option<String>> {
        // REQ-AXO-271 slice 2 (post-MIL-AXO-017 / DEC-AXO-083 AGE retirement) :
        // the legacy GraphProjection cache refresh via SQL CALLS / CALLS_NIF
        // tables was conditional on `skip_legacy_relations()`, which always
        // returns true under PG canonical. Authoritative call-graph reads
        // now route through `ist.Edge` + db/ddl/04_graph_functions.sql
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

    pub fn execute(&self, query: &str) -> Result<()> {
        // REQ-AXO-901881 W2 — native deadpool execute (was the FFI exec_fn).
        // REQ-AXO-902075 — surface the real PG error, not just the query text.
        self.pool
            .native
            .run_execute(query)
            .map_err(|e| anyhow!("Writer Error: {e} | query: {query}"))
    }

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<()> {
        let expanded = Self::expand_named_params(query, params)?;
        self.execute(&expanded)
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        self.query_json_on_writer(query)
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        let expanded = Self::expand_named_params(query, params)?;
        self.query_json_on_writer(&expanded)
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
        self.query_count_on_writer(query)
    }

    // ===================================================================
    // REQ-AXO-901884 Stage 0 — async-native typed read layer (ADDITIVE).
    // Routes to the NativePgCtx async core (no run_blocking hop, no
    // Vec<Vec<String>> render) and maps the structured PgError onto the
    // canonical "Graph plugin error: …" anyhow message. Consumers migrate
    // to these `.await` methods cluster-by-cluster (stages 1-6); the sync
    // query_json*/query_count* facade above is deleted once unused.
    // ===================================================================

    /// Async row read — typed `tokio_postgres::Row`s for `FromRow`/`try_get`.
    pub async fn query_rows(&self, sql: &str) -> Result<Vec<tokio_postgres::Row>> {
        self.pool
            .native
            .query(sql)
            .await
            .map_err(Self::pg_to_anyhow)
    }

    /// Async typed read: rows decoded into `T: FromRow`.
    pub async fn query_typed<T: FromRow>(&self, sql: &str) -> Result<Vec<T>> {
        let rows = self.query_rows(sql).await?;
        rows.iter().map(T::from_row).collect()
    }

    /// Async scalar count (`SELECT count(*)`): first column of the first row as
    /// i64, 0 when empty. The query must return a `bigint` first column (native
    /// type — no `::text` cast needed unlike the legacy render path).
    pub async fn query_count_async(&self, sql: &str) -> Result<i64> {
        let rows = self.query_rows(sql).await?;
        Ok(rows
            .first()
            .and_then(|r| r.try_get::<_, i64>(0).ok())
            .unwrap_or(0))
    }

    /// Async single scalar of an arbitrary `FromSql` type — first column of the
    /// first row, `None` when empty. Folds `query_single_i64_writer`,
    /// `pg_database_size_bytes`, `pg_wal_bytes`, etc.
    pub async fn query_scalar<T>(&self, sql: &str) -> Result<Option<T>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let rows = self.query_rows(sql).await?;
        match rows.first() {
            Some(row) => Ok(row.try_get::<_, Option<T>>(0)?),
            None => Ok(None),
        }
    }

    /// Async ANN (HNSW) typed-row read for the `retrieve_context` semantic lane.
    pub async fn query_ann_rows(
        &self,
        sql: &str,
        ef_search: u32,
    ) -> Result<Vec<tokio_postgres::Row>> {
        self.pool
            .native
            .query_ann(sql, ef_search)
            .await
            .map_err(Self::pg_to_anyhow)
    }

    /// Async multi-statement execute (writes / DDL / BEGIN…COMMIT batches).
    pub async fn execute_async(&self, sql: &str) -> Result<()> {
        self.pool
            .native
            .execute_batch_async(sql)
            .await
            .map_err(Self::pg_to_anyhow)
    }

    /// REQ-AXO-901884 — map the structured native `PgError` onto the canonical
    /// "Graph plugin error: <message>[ — <detail>]" anyhow message, preserving
    /// the prefix + SQLSTATE/hint that the sync `decode_native_envelope` form
    /// (and the callers/tests matching on it) surface today.
    fn pg_to_anyhow(e: crate::postgres::native::PgError) -> anyhow::Error {
        let mut detail = String::new();
        if let Some(code) = &e.detail.code {
            detail.push_str(&format!("[SQLSTATE {code}]"));
        }
        if let Some(hint) = &e.detail.hint {
            if !detail.is_empty() {
                detail.push_str(" — ");
            }
            detail.push_str(&format!("hint: {hint}"));
        }
        if detail.is_empty() {
            anyhow!("Graph plugin error: {}", e.detail.message)
        } else {
            anyhow!("Graph plugin error: {} — {}", e.detail.message, detail)
        }
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
        self.query_single_i64_writer("SELECT pg_total_relation_size('ist.ChunkEmbedding')::BIGINT")
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
        // REQ-AXO-244 — join the batch into one `BEGIN; …; COMMIT;` string so
        // the whole sequence runs on ONE connection: native `batch_execute`
        // pins a single pooled connection per call, so the BEGIN/COMMIT
        // pairing holds (no "idle in transaction" leak). batch_execute aborts
        // the transaction on any statement error, so no explicit ROLLBACK is
        // needed — the connection returns to the pool already rolled back.
        let mut combined =
            String::with_capacity(queries.iter().map(|q| q.len() + 2).sum::<usize>() + 32);
        combined.push_str("BEGIN;\n");
        for q in queries {
            combined.push_str(q);
            if !q.trim_end().ends_with(';') {
                combined.push(';');
            }
            combined.push('\n');
        }
        combined.push_str("COMMIT;");

        self.pool.native.run_execute(&combined).map_err(|e| {
            anyhow!("Batch Writer Error (size={}): {e}", queries.len())
        })
    }

    /// REQ-AXO-901881 W2 — native query dispatcher (was `query_on_ctx`, the
    /// FFI `query_json_fn` + CStr marshalling + writer ctx). `run_query_json`
    /// returns the rendered JSON array on success, or the REQ-AXO-129 envelope
    /// string (leading `{`) on error; the envelope→Err detection below is
    /// unchanged (kept byte-identical so the contract + the "Graph plugin
    /// error" message callers/tests match on are preserved).
    pub(crate) fn query_native(&self, query: &str) -> Result<String> {
        Self::decode_native_envelope(self.pool.native.run_query_json(query))
    }

    /// REQ-AXO-901883 — ANN (HNSW) semantic read for `retrieve_context` /
    /// `retrieve_context_v2`. Routes the ANN-CTE SELECT through
    /// `run_ann_query_json`, which scopes `SET LOCAL enable_seqscan=off` +
    /// `hnsw.ef_search` to a transaction so pgvector picks
    /// `chunk_embedding_hnsw_idx` regardless of table size. The
    /// success/error envelope contract is identical to `query_native`.
    pub(crate) fn query_ann_json(&self, query: &str, ef_search: u32) -> Result<String> {
        Self::decode_native_envelope(self.pool.native.run_ann_query_json(query, ef_search))
    }

    /// REQ-AXO-902129 — exact scoped vector scan (no HNSW), for bounded scopes where
    /// a corrupt/unreachable HNSW graph would return a tiny arbitrary pocket.
    pub(crate) fn query_exact_scan_json(&self, query: &str) -> Result<String> {
        Self::decode_native_envelope(self.pool.native.run_exact_scan_query_json(query))
    }

    /// REQ-AXO-129 envelope decoder shared by the native read paths.
    /// Legitimate results are a JSON array (`[`); error envelopes are a JSON
    /// object (`{`) carrying `_axon_plugin_error`. Surfaces the rich
    /// pg_error.message/code/hint so LLM callers see the real
    /// `column "X" does not exist` / SQLSTATE / hint, not a silent [].
    fn decode_native_envelope(res: String) -> Result<String> {
        if res.starts_with('{') {
            if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&res) {
                if let Some(message) = envelope.get("_axon_plugin_error").and_then(|v| v.as_str()) {
                    let pg_msg = envelope
                        .pointer("/pg_error/message")
                        .and_then(|v| v.as_str());
                    let pg_code = envelope.pointer("/pg_error/code").and_then(|v| v.as_str());
                    let pg_hint = envelope.pointer("/pg_error/hint").and_then(|v| v.as_str());
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
                        return Err(anyhow::anyhow!("Graph plugin error: {message}"));
                    }
                    return Err(anyhow::anyhow!("Graph plugin error: {message} — {detail}"));
                }
            }
        }
        Ok(res)
    }

    /// REQ-AXO-901995 — render a string value as a PG literal via dollar quoting,
    /// which needs NO escaping of apostrophes, backslashes OR backticks. The old
    /// `'{}'.replace('\'', "''")` only escaped apostrophes; JSON payloads
    /// (entrench `metadata.nuances`, acceptance_criteria with backticks/escapes)
    /// also carry backslashes, which broke `entrench_nuance(confirm=true)` with a
    /// Writer Error on very common text. The dollar tag is grown until it does not
    /// occur in the value, so the literal is always well-formed.
    fn pg_str_literal(v: &str) -> String {
        let mut n = 0u32;
        loop {
            let tag = if n == 0 {
                "$axp$".to_string()
            } else {
                format!("$axp{n}$")
            };
            if !v.contains(&tag) {
                return format!("{tag}{v}{tag}");
            }
            n += 1;
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
                        let value = iter
                            .next()
                            .ok_or_else(|| anyhow!("Too few positional parameters supplied"))?;
                        let replacement = match value {
                            serde_json::Value::Null => "NULL".to_string(),
                            serde_json::Value::Bool(v) => v.to_string(),
                            serde_json::Value::Number(v) => v.to_string(),
                            serde_json::Value::String(v) => Self::pg_str_literal(v),
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
                serde_json::Value::String(v) => Self::pg_str_literal(v),
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

/// Produce a lexical *skeleton* of `sql`: comments, string literals, quoted
/// identifiers and dollar-quoted bodies are each replaced by a single space so
/// the read-only guard can scan bare keyword tokens without tripping on a
/// keyword that merely appears inside a literal or identifier.
///
/// A single left-to-right pass is required — a `'` inside a comment is not a
/// string, and a `--` inside a string is not a comment.
/// REQ-AXO-902077 — a leading `-- comment` / `/* block */` before a SELECT was
/// wrongly rejected by a bare first-token match; comments must be elided first.
/// REQ-AXO-902207 — a keyword inside a string literal (`SELECT 'insert' AS c`)
/// or a double-quoted identifier (`SELECT "update" FROM t`) is NOT a write, so
/// those spans are elided before the mutation scan to avoid false positives.
fn sql_skeleton(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // line comment `-- … \n`
            '-' if chars.peek() == Some(&'-') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n'); // keep the newline as a token separator
                        break;
                    }
                }
            }
            // block comment `/* … */`
            '/' if chars.peek() == Some(&'*') => {
                chars.next(); // consume '*'
                let mut prev = ' ';
                for d in chars.by_ref() {
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
                out.push(' '); // separator so neighbouring tokens don't fuse
            }
            // single-quoted string literal (`''` escapes an embedded quote)
            '\'' => {
                while let Some(d) = chars.next() {
                    if d == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            continue;
                        }
                        break;
                    }
                }
                out.push(' ');
            }
            // double-quoted identifier (`""` escapes an embedded quote)
            '"' => {
                while let Some(d) = chars.next() {
                    if d == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            continue;
                        }
                        break;
                    }
                }
                out.push(' ');
            }
            // dollar-quoted string `$tag$ … $tag$` (tag ∈ [A-Za-z0-9_]*).
            // `$1` / `$sym` are positional / named params, NOT dollar quotes.
            '$' => {
                let mut tag = String::new();
                let mut opened = false;
                loop {
                    match chars.peek() {
                        Some(&'$') => {
                            chars.next();
                            opened = true;
                            break;
                        }
                        Some(&d) if d.is_ascii_alphanumeric() || d == '_' => {
                            tag.push(d);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                if opened {
                    let closing = format!("${tag}$");
                    let mut window = String::new();
                    for d in chars.by_ref() {
                        window.push(d);
                        if window.ends_with(&closing) {
                            break;
                        }
                    }
                    out.push(' ');
                } else {
                    // not a dollar quote: re-emit `$` + the consumed tag chars
                    out.push('$');
                    out.push_str(&tag);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// REQ-AXO-902077 / REQ-AXO-902207 — read-only guard by light lexing, not a
/// bare first-token match. The `sql` MCP tool is strictly read-only, so every
/// `;`-separated statement must independently satisfy BOTH:
///   1. its leading keyword is a read verb
///      (select / with / pragma / show / describe / explain), and
///   2. it embeds no data-modifying token — `insert` / `update` / `delete` /
///      `merge` (the statement kinds PostgreSQL allows inside a `WITH` clause
///      and the only ones `EXPLAIN ANALYZE` executes) nor a bare `into`
///      (`SELECT … INTO newtbl` creates a table, the `CREATE TABLE AS` cousin).
/// A DDL / admin verb (drop / alter / truncate / …) cannot live inside a CTE or
/// sub-query, so it is only reachable at a statement head and is caught by (1).
/// `sql_skeleton` elides literals / identifiers first, so `SELECT 'insert' AS c`
/// or `SELECT "update" FROM t` are NOT false positives; and the `FOR [NO KEY]
/// UPDATE` row-locking clause (a read) is not mistaken for an `UPDATE` write.
///
/// Known limitation: keyword scanning cannot see a write performed by a
/// side-effecting function call (e.g. `SELECT nextval(...)` or
/// `SELECT a_function_that_inserts()`); such writes are out of scope here.
pub(crate) fn is_read_only_sql(query: &str) -> bool {
    const READ_VERBS: &[&str] =
        &["select", "with", "pragma", "show", "describe", "explain"];
    // Write signals that may appear AFTER a read leading verb: the
    // data-modifying statements PostgreSQL allows inside a `WITH` clause plus
    // the table-creating `SELECT … INTO newtbl`. `update` is handled separately
    // below because the `FOR [NO KEY] UPDATE` row-locking clause is a *read*
    // that also carries the bare `update` token.
    const ALWAYS_WRITE_TOKENS: &[&str] = &["insert", "delete", "merge", "into"];

    let skeleton = sql_skeleton(query).to_ascii_lowercase();

    let mut saw_statement = false;
    for statement in skeleton.split(';') {
        let mut tokens = statement
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty());
        let Some(first) = tokens.next() else {
            continue; // empty fragment (e.g. a trailing `;`)
        };
        saw_statement = true;
        if !READ_VERBS.contains(&first) {
            return false;
        }
        // Scan the remaining tokens for an embedded write. `update` counts only
        // when it is NOT the tail of a `FOR UPDATE` / `FOR NO KEY UPDATE`
        // locking clause (those modify no data); a data-modifying `UPDATE` CTE
        // is preceded by `as` / `(`, never by `for` / `key`.
        let mut prev = first;
        for w in tokens {
            let is_write = ALWAYS_WRITE_TOKENS.contains(&w)
                || (w == "update" && prev != "for" && prev != "key");
            if is_write {
                return false;
            }
            prev = w;
        }
    }
    saw_statement
}

// REQ-AXO-091 placeholder-expansion tests live in a sibling file so the
// commit's diff path satisfies the TDD guideline (GUI-PRO-001) which
// expects a `_tests.rs` companion path.
#[cfg(test)]
#[path = "graph_query_tests.rs"]
mod expand_params_tests;

#[cfg(test)]
mod tests {
    use crate::graph::GraphStore;

    #[test]
    fn read_only_guard_strips_comments_and_blocks_injection() {
        use super::is_read_only_sql;
        // REQ-AXO-902077 — the real friction: a leading comment before a SELECT.
        assert!(is_read_only_sql("-- a comment\nSELECT 1"));
        assert!(is_read_only_sql("/* block */ SELECT 1"));
        assert!(is_read_only_sql("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(is_read_only_sql("SELECT 'a;b' AS x")); // `;` in a string literal
        // mutations + injection-after-semicolon stay rejected.
        assert!(!is_read_only_sql("DELETE FROM t"));
        assert!(!is_read_only_sql("-- x\nUPDATE t SET a=1"));
        assert!(!is_read_only_sql("SELECT 1; DROP TABLE t"));
        assert!(!is_read_only_sql("SELECT 1 ; truncate t"));
    }

    #[test]
    fn read_only_guard_rejects_embedded_writes_req_902207() {
        use super::is_read_only_sql;
        // REQ-AXO-902207 — a data-modifying CTE writes despite a SELECT/WITH
        // prefix; the pre-hardening guard only scanned statements AFTER a `;`,
        // so these all sailed through as "read-only". Each MUST be rejected.
        assert!(!is_read_only_sql(
            "WITH x AS (INSERT INTO t VALUES (1) RETURNING *) SELECT * FROM x"
        ));
        assert!(!is_read_only_sql(
            "WITH d AS (DELETE FROM t WHERE id = 1 RETURNING id) SELECT count(*) FROM d"
        ));
        assert!(!is_read_only_sql(
            "WITH u AS (UPDATE t SET a = 1 WHERE id = 2 RETURNING *) SELECT * FROM u"
        ));
        assert!(!is_read_only_sql(
            "WITH m AS (MERGE INTO t USING s ON t.id = s.id \
             WHEN MATCHED THEN UPDATE SET a = 1) SELECT 1"
        ));
        // nested writable CTE + mixed case.
        assert!(!is_read_only_sql(
            "WITH outer_cte AS (WITH inner_cte AS (delete from t returning id) \
             SELECT * FROM inner_cte) SELECT * FROM outer_cte"
        ));
        // `EXPLAIN ANALYZE <dml>` actually executes the write.
        assert!(!is_read_only_sql("EXPLAIN ANALYZE INSERT INTO t VALUES (1)"));
        // `SELECT … INTO newtbl` creates a table (the `CREATE TABLE AS` cousin).
        assert!(!is_read_only_sql("SELECT * INTO new_table FROM t"));
        assert!(!is_read_only_sql("SELECT id INTO t2 FROM t1 WHERE id > 0"));
        // write embedded in a second statement after a `;`.
        assert!(!is_read_only_sql(
            "SELECT 1; WITH x AS (INSERT INTO t VALUES (1) RETURNING id) SELECT * FROM x"
        ));

        // False-positive guard — a write keyword that only appears inside a
        // string literal, a quoted identifier, a dollar-quoted body, or as a
        // substring of a longer identifier is NOT a write and stays accepted.
        assert!(is_read_only_sql("SELECT 'insert' AS col"));
        assert!(is_read_only_sql("SELECT 'delete from t' AS note"));
        assert!(is_read_only_sql("SELECT \"insert\" FROM t")); // quoted identifier
        assert!(is_read_only_sql("SELECT $$delete$$ AS x")); // dollar-quoted body
        assert!(is_read_only_sql("SELECT $tag$update t$tag$ AS x")); // tagged dollar-quote
        assert!(is_read_only_sql("SELECT col AS updated_at FROM t")); // substring, not a token
        assert!(is_read_only_sql("SELECT created_at, deleted_flag FROM t"));
        assert!(is_read_only_sql("WITH x AS (SELECT 1) SELECT * FROM x")); // read-only CTE
        // a non-reserved keyword used as an ordinary column mid-statement is
        // not a write and must not become a false positive.
        assert!(is_read_only_sql("SELECT copy, refresh FROM audit_log"));
        // `FOR [NO KEY] UPDATE` / `FOR SHARE` are row-locking *reads* — the
        // bare `update` token there must NOT trigger a rejection.
        assert!(is_read_only_sql("SELECT id FROM t WHERE x = 1 FOR UPDATE"));
        assert!(is_read_only_sql("SELECT * FROM t FOR NO KEY UPDATE"));
        assert!(is_read_only_sql("SELECT * FROM t FOR SHARE"));
        // a read-only CTE may itself carry a locking clause and stay read-only,
        // while a sibling data-modifying UPDATE CTE is still caught.
        assert!(is_read_only_sql(
            "WITH a AS (SELECT * FROM t FOR UPDATE) SELECT * FROM a"
        ));
        assert!(!is_read_only_sql(
            "WITH a AS (SELECT 1 FOR UPDATE), b AS (UPDATE t SET c = 1 RETURNING *) \
             SELECT * FROM b"
        ));
    }

    #[test]
    fn execute_raw_sql_gateway_supports_read_only_and_mutating_queries() {
        // REQ-AXO-901653 slice-5c — File table dropped ; test migrated to
        // ist.IndexedFile (3 cols : path, content_hash, last_seen_ms).
        let store = crate::tests::test_helpers::create_test_db().unwrap();

        let read = store.execute_raw_sql_gateway("SELECT 1").unwrap();
        assert!(read.contains("1"), "{read}");

        let write = store
            .execute_raw_sql_gateway(
                "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) \
                 VALUES ('/tmp/sql_gateway.ex', 'AXO', 'hash-1', 1)",
            )
            .unwrap();
        assert!(write.contains("\"ok\":true"), "{write}");

        let count = store
            .query_count("SELECT count(*) FROM ist.IndexedFile WHERE path = '/tmp/sql_gateway.ex'")
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn render_pg_value_decodes_temporal_and_uuid_types() {
        // REQ-AXO-901960 — the temporal family used to fall through to the
        // `<unsupported type ...>` sentinel, so e.g.
        // `axon.mcp_friction.last_observed_at` (timestamptz) was unreadable via
        // the `sql` tool. They now render their real value.
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let res = store
            .query_json(
                "SELECT \
                   '2026-06-12T13:00:00Z'::timestamptz AS ts, \
                   '2026-06-12 13:00:00'::timestamp AS tsn, \
                   '2026-06-12'::date AS d, \
                   '13:00:00'::time AS t, \
                   '3.14'::numeric AS n, \
                   sum(x)::numeric AS s FROM (VALUES (1::bigint),(2)) v(x)",
            )
            .unwrap();
        assert!(
            !res.contains("<unsupported type"),
            "sentinel leaked for a temporal/numeric value: {res}"
        );
        assert!(
            res.contains("2026-06-12"),
            "timestamptz/timestamp/date value missing: {res}"
        );
        assert!(res.contains("13:00:00"), "time value missing: {res}");
        // REQ-AXO-901905 — numeric (literal + sum(bigint)) renders natively now.
        assert!(res.contains("3.14"), "numeric literal value missing: {res}");
        assert!(res.contains('3'), "sum(bigint)::numeric value missing: {res}");
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
