//! REQ-AXO-142 — IST/SOLL test fixtures for TDD-friendly contributor tooling.
//!
//! Builds Symbol / Edge (CALLS, CONTAINS, …) / soll.Node rows in production
//! format and seeds them into an **isolated** [`GraphStore`] — a per-test
//! clone of the canonical test template (`axon_test_template`), never the
//! shared `axon_dev` database. Lets contributors TDD changes to IST/SOLL
//! projection logic with the same column names, types and edge shapes the
//! live indexer emits.
//!
//! ## Example
//!
//! ```ignore
//! use crate::test_support::ist_fixtures::{
//!     create_test_server_with_ist_seed, CallFixture, IstSeed, SymbolFixture,
//! };
//!
//! let harness = create_test_server_with_ist_seed(
//!     IstSeed::new()
//!         .symbol(SymbolFixture::new("prj::core_func", "core_func", "function", "PRJ").tested(true))
//!         .symbol(SymbolFixture::new("prj::caller_func", "caller_func", "function", "PRJ"))
//!         .call(CallFixture::canonical("prj::caller_func", "prj::core_func", "PRJ")),
//! )
//! .unwrap();
//! // harness.server / harness.store available; tempdir cleans up on Drop.
//! ```
//!
//! ## Known parity quirks
//!
//! - **CALLS.target_id format.** The live IST indexer emits two forms of
//!   `CALLS` edges (rows of `ist.Edge` with `relation_type='CALLS'`) that
//!   downstream queries must handle:
//!   1. canonical Symbol.id (e.g. `axon::core_func`)
//!   2. synthetic `<caller_file>::<callee_name>` for cross-module Rust impl
//!      method calls (REQ-AXO-134). `tools_dx::inspect_callers_query`
//!      compensates with `target_id LIKE ('%::' || s.name)` in a correlated
//!      subquery. Tests that exercise that branch must seed BOTH forms via
//!      [`CallFixture::canonical`] and [`CallFixture::synthetic`].
//!
//! - **Read-after-write.** [`seed_ist`] writes through the PG writer pool ;
//!   subsequent reads observe all inserted rows immediately under MVCC
//!   (REQ-AXO-901870 retired the DuckDB reader-replica + its refresh step).

use std::sync::Arc;

use anyhow::Result;
use tempfile::TempDir;

use crate::graph::GraphStore;
use crate::test_support::test_db::TestDb;

/// Builder for one row of `Symbol (id, name, kind, tested, is_public, is_nif,
/// is_unsafe, project_code, embedding)`. Embedding is left NULL — symbol
/// projection tests do not depend on it; callers that need a vector should
/// extend this builder rather than insert ad-hoc SQL.
pub struct SymbolFixture {
    id: String,
    name: String,
    kind: String,
    project_code: String,
    tested: bool,
    is_public: bool,
    is_nif: bool,
    is_unsafe: bool,
}

impl SymbolFixture {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: impl Into<String>,
        project_code: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            project_code: project_code.into(),
            tested: false,
            is_public: true,
            is_nif: false,
            is_unsafe: false,
        }
    }

    pub fn tested(mut self, tested: bool) -> Self {
        self.tested = tested;
        self
    }

    pub fn is_public(mut self, is_public: bool) -> Self {
        self.is_public = is_public;
        self
    }

    pub fn is_nif(mut self, is_nif: bool) -> Self {
        self.is_nif = is_nif;
        self
    }

    pub fn is_unsafe(mut self, is_unsafe: bool) -> Self {
        self.is_unsafe = is_unsafe;
        self
    }

    fn insert_sql(&self) -> String {
        format!(
            "INSERT INTO ist.Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) \
             VALUES ('{}', '{}', '{}', {}, {}, {}, {}, '{}')",
            sql_escape(&self.id),
            sql_escape(&self.name),
            sql_escape(&self.kind),
            self.tested,
            self.is_public,
            self.is_nif,
            self.is_unsafe,
            sql_escape(&self.project_code),
        )
    }
}

/// Builder for one row of `CALLS (source_id, target_id, project_code)`.
///
/// Use [`CallFixture::canonical`] for the Symbol.id-matching form and
/// [`CallFixture::synthetic`] for the `<file>::<name>` form the indexer
/// emits for cross-module Rust impl method calls (REQ-AXO-134).
pub struct CallFixture {
    source_id: String,
    target_id: String,
    project_code: String,
}

impl CallFixture {
    pub fn canonical(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        project_code: impl Into<String>,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            target_id: target_id.into(),
            project_code: project_code.into(),
        }
    }

    /// REQ-AXO-134 — synthetic `<caller_file>::<callee_name>` form emitted by
    /// the IST indexer for cross-module Rust impl method calls. The
    /// `inspect_callers_query` LIKE-with-`||` workaround matches this shape.
    pub fn synthetic(
        source_id: impl Into<String>,
        caller_file: impl AsRef<str>,
        callee_name: impl AsRef<str>,
        project_code: impl Into<String>,
    ) -> Self {
        let target_id = format!("{}::{}", caller_file.as_ref(), callee_name.as_ref());
        Self {
            source_id: source_id.into(),
            target_id,
            project_code: project_code.into(),
        }
    }

    fn insert_sql(&self) -> String {
        // Post-AGE-retirement: CALLS is a `relation_type` on the unified
        // `ist.Edge` table, not a standalone table. `created_at_ms` is NOT
        // NULL; fixtures use 0 (epoch) since ordering is irrelevant in tests.
        format!(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
             VALUES ('{}', '{}', 'CALLS', '{}', 0)",
            sql_escape(&self.source_id),
            sql_escape(&self.target_id),
            sql_escape(&self.project_code),
        )
    }
}

/// Builder for one row of `soll.Node (id, type, project_code, title,
/// description, status, metadata)`.
pub struct SollNodeFixture {
    id: String,
    ty: String,
    project_code: String,
    title: String,
    description: String,
    status: String,
    metadata: String,
}

impl SollNodeFixture {
    pub fn new(
        id: impl Into<String>,
        ty: impl Into<String>,
        project_code: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            ty: ty.into(),
            project_code: project_code.into(),
            title: title.into(),
            description: String::new(),
            status: "current".to_string(),
            metadata: "{}".to_string(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    pub fn metadata_json(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = metadata.into();
        self
    }

    fn insert_sql(&self) -> String {
        format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('{}', '{}', '{}', '{}', '{}', '{}', '{}')",
            sql_escape(&self.id),
            sql_escape(&self.ty),
            sql_escape(&self.project_code),
            sql_escape(&self.title),
            sql_escape(&self.description),
            sql_escape(&self.status),
            sql_escape(&self.metadata),
        )
    }
}

/// Builder for one IST edge of any `relation_type` on the unified
/// `ist.Edge` table: `CONTAINS`, `IMPACTS`, `SUBSTANTIATES`, `CALLS_NIF`.
/// The `table` argument is the `relation_type`. Use [`CallFixture`] for the
/// `CALLS` relation (it also exposes the synthetic-target form).
pub struct EdgeFixture {
    /// Becomes `ist.Edge.relation_type`. Named `table` for backward source
    /// compatibility with the AGE-era per-relation-table fixtures.
    table: String,
    source_id: String,
    target_id: String,
    project_code: String,
}

impl EdgeFixture {
    pub fn new(
        table: impl Into<String>,
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        project_code: impl Into<String>,
    ) -> Self {
        Self {
            table: table.into(),
            source_id: source_id.into(),
            target_id: target_id.into(),
            project_code: project_code.into(),
        }
    }

    fn insert_sql(&self) -> String {
        format!(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
             VALUES ('{}', '{}', '{}', '{}', 0)",
            sql_escape(&self.source_id),
            sql_escape(&self.target_id),
            sql_escape(&self.table),
            sql_escape(&self.project_code),
        )
    }
}

/// Bundle of fixtures to seed in one call.
#[derive(Default)]
pub struct IstSeed {
    pub symbols: Vec<SymbolFixture>,
    pub calls: Vec<CallFixture>,
    pub nodes: Vec<SollNodeFixture>,
    pub edges: Vec<EdgeFixture>,
}

impl IstSeed {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn symbol(mut self, fixture: SymbolFixture) -> Self {
        self.symbols.push(fixture);
        self
    }

    pub fn call(mut self, fixture: CallFixture) -> Self {
        self.calls.push(fixture);
        self
    }

    pub fn node(mut self, fixture: SollNodeFixture) -> Self {
        self.nodes.push(fixture);
        self
    }

    pub fn edge(mut self, fixture: EdgeFixture) -> Self {
        self.edges.push(fixture);
        self
    }

    /// REQ-AXO-902075 — distinct project codes referenced by this seed. The
    /// per-test PG is fresh, but the IST RAM snapshot cache is PROCESS-global:
    /// a sibling test may have published a snapshot for one of these codes and
    /// never evicted it, so a reader (inspect / path / resolve) would serve that
    /// stale snapshot instead of this test's freshly-seeded PG. The harness
    /// evicts these before handing back the server (test-isolation, REQ-901721
    /// family).
    pub fn project_codes(&self) -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        for s in &self.symbols {
            set.insert(s.project_code.clone());
        }
        for c in &self.calls {
            set.insert(c.project_code.clone());
        }
        for n in &self.nodes {
            set.insert(n.project_code.clone());
        }
        for e in &self.edges {
            set.insert(e.project_code.clone());
        }
        set
    }
}

/// Seed all fixtures in the bundle into `store`. Writes go through the PG
/// writer pool; subsequent reads observe them immediately under MVCC
/// (REQ-AXO-901870 retired the DuckDB reader-replica + its refresh step).
pub fn seed_ist(store: &GraphStore, seed: &IstSeed) -> Result<()> {
    for fixture in &seed.symbols {
        store.execute(&fixture.insert_sql())?;
    }
    for fixture in &seed.calls {
        store.execute(&fixture.insert_sql())?;
    }
    for fixture in &seed.nodes {
        store.execute(&fixture.insert_sql())?;
    }
    for fixture in &seed.edges {
        store.execute(&fixture.insert_sql())?;
    }
    Ok(())
}

/// Assert exact row count for a count-shaped query (`SELECT count(*) ...`).
#[track_caller]
pub fn assert_ist_count(store: &GraphStore, sql: &str, expected: i64) {
    let actual = store
        .query_count(sql)
        .unwrap_or_else(|err| panic!("query_count failed for `{sql}`: {err}"));
    assert_eq!(
        actual, expected,
        "ist count mismatch for `{sql}`: expected {expected}, got {actual}",
    );
}

/// Test harness owning the tempdir, the [`GraphStore`] and the
/// [`crate::mcp::McpServer`]. The tempdir is removed on Drop.
pub struct TestServerHarness {
    pub server: crate::mcp::McpServer,
    pub store: Arc<GraphStore>,
    _tempdir: TempDir,
    /// Declared last so it drops last: `server` then `store` release the PG
    /// pool first, then `TestDb`'s Drop best-effort `dropdb`s the isolated
    /// clone. Leaks are reclaimed by the pre-run sweep regardless.
    _test_db: TestDb,
}

/// Build an **isolated** [`GraphStore`] (a fresh clone of the canonical test
/// template), wrap it in an [`crate::mcp::McpServer`], and seed `seed` before
/// returning the harness.
pub fn create_test_server_with_ist_seed(seed: IstSeed) -> Result<TestServerHarness> {
    let tempdir = tempfile::tempdir()?;
    let db_root = tempdir
        .path()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("tempdir path is not valid UTF-8"))?
        .to_string();
    // REQ-AXO-91560 — clone the canonical test template (DDL + global seed +
    // auto-seed triggers) via `new_with_database`. The previous
    // `GraphStore::new` path resolved to the shared `axon_dev` database, which
    // lacked the test triggers (FK failures on Symbol/Edge/Chunk) and leaked
    // fixture writes across tests and into dev.
    let test_db = TestDb::create();
    let store = Arc::new(GraphStore::new_with_database(&db_root, &test_db.url())?);
    seed_ist(&store, &seed)?;
    // REQ-AXO-902075 — drop any stale process-global RAM IST snapshot for the
    // seeded projects so readers fall through to THIS test's PG (lazy-warmed),
    // not a snapshot a sibling test left warm (fixes the order-dependent
    // `test_axon_inspect` flake; test-isolation REQ-901721 family).
    for code in seed.project_codes() {
        crate::ist_snapshot::evict_process_snapshot(&code);
    }
    let server = crate::mcp::McpServer::new(store.clone());
    Ok(TestServerHarness {
        server,
        store,
        _tempdir: tempdir,
        _test_db: test_db,
    })
}

fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
#[path = "ist_fixtures_tests.rs"]
mod ist_fixtures_tests;
