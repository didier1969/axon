// Copyright (c) Didier Stadelmann. All rights reserved.

//! REQ-AXO-91560 / REQ-AXO-91562 — canonical per-test PostgreSQL isolation.
//!
//! Single home for the ephemeral-database harness shared by every test that
//! needs an isolated, seeded store: the raw-SQL tests under
//! `crate::mcp::tests` and the IST/SOLL builder fixtures under
//! [`crate::test_support::ist_fixtures`]. Each [`TestDb`] is a fresh
//! `createdb -T axon_test_template` clone carrying the canonical DDL + global
//! seed + the test-only auto-seed triggers, so fixtures insert IST/SOLL rows
//! without hand-seeding FK parents and without ever touching the shared
//! `axon_dev` database (the historical `GraphStore::new` path that leaked test
//! writes into dev and broke isolation — REQ-AXO-901718/720/721 root cause).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Test-cluster port (devenv PG). Overridden by `PGPORT`.
pub(crate) fn pg_port() -> String {
    std::env::var("PGPORT").unwrap_or_else(|_| "44144".to_string())
}

fn template_name() -> String {
    std::env::var("AXON_TEST_TEMPLATE").unwrap_or_else(|_| "axon_test_template".to_string())
}

/// REQ-AXO-901873 — `dropdb --force --if-exists` : termine les connexions
/// résiduelles puis DROP. Remplace le `dropdb` best-effort qui leakait dès
/// qu'une connexion subsistait. Renvoie `true` si la base n'existe plus après.
fn force_dropdb(db_name: &str, pg_port: &str) -> bool {
    std::process::Command::new("dropdb")
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            pg_port,
            "-U",
            "axon",
            "--force",
            "--if-exists",
            db_name,
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// REQ-AXO-901873 — registre des bases créées par CE process, force-droppées à
/// la sortie du process via un hook `libc::atexit`. Garantit la suppression
/// systématique **à la fin du run** même pour les guards parkés en `static`
/// (qui ne déclenchent jamais `Drop`). Complète le `Drop` per-test (fast-path)
/// et le pre-run sweep (fallback terminaison anormale).
fn registered_test_dbs() -> &'static Mutex<Vec<(String, String)>> {
    static REGISTERED: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    REGISTERED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Enregistre `(db_name, pg_port)` pour la réclamation de fin de process et
/// arme le hook `atexit` une seule fois.
fn register_for_atexit_cleanup(db_name: &str, pg_port: &str) {
    static ARMED: OnceLock<()> = OnceLock::new();
    if let Ok(mut v) = registered_test_dbs().lock() {
        v.push((db_name.to_string(), pg_port.to_string()));
    }
    ARMED.get_or_init(|| {
        // SAFETY: `drop_registered_test_dbs` est une `extern "C" fn` sans état
        // capturé ; elle lit le registre process-global. Armée une seule fois.
        unsafe {
            libc::atexit(drop_registered_test_dbs);
        }
    });
}

/// Handler `libc::atexit` — s'exécute à la terminaison normale du process.
/// Force-DROP chaque base `axon_test_*` créée par ce process. Best-effort
/// (le process sort de toute façon).
extern "C" fn drop_registered_test_dbs() {
    let dbs: Vec<(String, String)> = match registered_test_dbs().lock() {
        Ok(v) => v.clone(),
        Err(p) => p.into_inner().clone(),
    };
    for (db_name, pg_port) in dbs {
        let _ = force_dropdb(&db_name, &pg_port);
    }
}

/// REQ-AXO-91562 Slice 2 — per-test database isolation via PG template.
///
/// Each test gets a fresh database cloned from `axon_test_template`.
///
/// Lifecycle / reclamation (REQ-AXO-901848): the `Drop` impl issues a
/// best-effort `dropdb`. Callers that park the guard in a process-lifetime
/// `static` never run `Drop` (Rust does not drop `static` contents at exit),
/// so the canonical reclamation is the idempotent, connection-safe pre-run
/// [`sweep_stale_test_databases`], invoked once per process the first time a
/// `TestDb` is created. It reclaims databases leaked by *previous* runs
/// regardless of how this process terminates. Callers that own the guard for a
/// single test's duration (e.g. the IST fixture harness) get the `Drop`
/// fast-path for free.
pub(crate) struct TestDb {
    db_name: String,
    pg_port: String,
}

impl TestDb {
    pub(crate) fn create() -> Self {
        // REQ-AXO-901848 — reclaim databases leaked by previous runs before
        // creating this run's database. Idempotent and connection-safe.
        let port = pg_port();
        sweep_once(&port);
        // REQ-AXO-91560 — bring the clone template to canonical schema+seed
        // (and test auto-seed triggers) before the first `createdb -T` below.
        ensure_template_once(&port);

        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tid = std::thread::current().id();
        let db_name = format!("axon_test_{:x}_{:?}", id, tid)
            .replace("ThreadId(", "t")
            .replace(')', "");
        let template = template_name();

        let output = std::process::Command::new("createdb")
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port,
                "-U",
                "axon",
                "-T",
                &template,
                &db_name,
            ])
            .output()
            .expect("createdb command failed to execute");

        if !output.status.success() {
            panic!(
                "TestDb create failed for {}: {}",
                db_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // REQ-AXO-901873 — réclamation systématique à la fin du run (couvre les
        // guards parkés en `static` qui ne déclenchent jamais `Drop`).
        register_for_atexit_cleanup(&db_name, &port);

        TestDb {
            db_name,
            pg_port: port,
        }
    }

    pub(crate) fn url(&self) -> String {
        format!(
            "postgres://axon@127.0.0.1:{}/{}",
            self.pg_port, self.db_name
        )
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // REQ-AXO-901873 — Drop fiable : `dropdb --force` termine les connexions
        // résiduelles puis DROP (l'ancien best-effort leakait dès qu'une
        // connexion subsistait). Erreur surfacée ; le hook `atexit` reprend en
        // filet de sécurité si ce Drop échoue.
        if !force_dropdb(&self.db_name, &self.pg_port) {
            eprintln!(
                "WARN REQ-AXO-901873: dropdb --force a échoué pour {} (le hook atexit retentera)",
                self.db_name
            );
        }
    }
}

/// REQ-AXO-901882 — process-shared disposable test database (harness guard).
///
/// Routes the legacy `create_test_db()` path off the production `axon_live` /
/// `axon_dev` instances (`postgres::resolve_database_url` defaults
/// `AXON_INSTANCE=live`, so the old `GraphStore::new` path silently read/wrote
/// the real SOLL knowledge base) onto ONE clone of `axon_test_template` per
/// process. A single shared DB — not a per-test clone — preserves the prior
/// shared-state concurrency model of `create_test_db` and avoids the per-test
/// `createdb` contention that broke `pipeline_v2` `stage_a3`; the canonical
/// per-test isolation of those sites is deferred to REQ-AXO-901877. Created
/// once per process and reclaimed by the same pre-run sweep + `atexit` path as
/// [`TestDb`].
pub(crate) fn shared_test_db_url() -> String {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let port = pg_port();
        sweep_once(&port);
        ensure_template_once(&port);
        let db_name = format!("axon_test_shared_{}", std::process::id());
        // Reclaim a stale same-pid DB (pid reuse after an abnormal exit), then
        // clone the canonical template.
        force_dropdb(&db_name, &port);
        let template = template_name();
        let output = std::process::Command::new("createdb")
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port,
                "-U",
                "axon",
                "-T",
                &template,
                &db_name,
            ])
            .output()
            .expect("createdb (shared test db) failed to execute");
        if !output.status.success() {
            panic!(
                "shared test db create failed for {}: {}",
                db_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        register_for_atexit_cleanup(&db_name, &port);
        let url = format!("postgres://axon@127.0.0.1:{}/{}", port, db_name);

        // REQ-AXO-901903 (REQ-AXO-901882 gap fix) — the bulk_writer (COPY BINARY,
        // the sole A3/B3 write path) builds its pool from `resolve_database_url`
        // (env), NOT from the per-store explicit url. Without this, A3 writes
        // (`upsert_graph_v2` → `flush_batch`) land in the env-resolved live/dev
        // DB while the test store reads the isolated test DB → 0 rows, so every
        // a3_enroll/upsert integration test silently validated nothing. Register
        // THIS shared test DB as the canonical resolver's test override so the
        // bulk_writer (and any other env-resolving consumer) targets it. Uses a
        // race-free `OnceLock` set (NOT `std::env::set_var`, which is unsound
        // under the parallel test runner — concurrent env mutation corrupted
        // other env-reading tests). Set before the first `flush_batch` (every
        // write path goes through a GraphStore built via `create_test_db`, which
        // calls this function first).
        crate::postgres::set_test_db_url_override(&url);
        url
    })
    .clone()
}

/// REQ-AXO-901848 — reclaim `axon_test_*` databases leaked by previous test
/// runs. Runs exactly once per test process (guarded by [`sweep_once`]) before
/// the first database is created.
///
/// Concurrency safety: only databases with **zero** active backends in
/// `pg_stat_activity` are dropped, so a database currently in use by a
/// parallel test binary is never touched. Fresh databases created by *this*
/// run carry unique nanosecond+thread-id names that cannot collide with the
/// leaked names being swept, so there is no create/sweep race. The template
/// (`axon_test_template`) and any non-test database are excluded by the
/// `LIKE 'axon\_test\_%'` filter plus an explicit guard.
pub(crate) fn sweep_stale_test_databases(pg_port: &str) {
    // `DROP DATABASE` cannot run inside a transaction block, so a DO/loop is
    // not an option; `\gexec` executes each generated statement as its own
    // top-level command. ON_ERROR_STOP=0 keeps one failed drop (e.g. a
    // database that acquired a connection between SELECT and DROP) from
    // aborting the rest.
    // REQ-AXO-901906 — exclude THIS process's own shared test DB. Since
    // `NativePgCtx::drop` now closes pools eagerly, the process-shared DB
    // (`axon_test_shared_<pid>`) sits at zero backends *between* tests; without
    // this guard the mid-run `sweep_reclaims_leaked_test_databases` test (which
    // re-invokes the real sweep) would DROP the live shared DB, and every
    // subsequent `create_test_db` — whose URL is memoised in a OnceLock — would
    // fail to connect ("pool init failed"). Other processes'/prior runs' shared
    // + per-test DBs (different names) are still reclaimed.
    let own_shared = format!("axon_test_shared_{}", std::process::id());
    let script = format!(
        "\\set ON_ERROR_STOP 0\n\
        SELECT format('DROP DATABASE IF EXISTS %I', d.datname)\n\
        FROM pg_database d\n\
        WHERE d.datname LIKE 'axon\\_test\\_%'\n\
          AND d.datname <> 'axon_test_template'\n\
          AND d.datname <> '{own_shared}'\n\
          AND NOT EXISTS (\n\
            SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname\n\
          )\n\
        \\gexec\n"
    );

    let mut child = match std::process::Command::new("psql")
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            pg_port,
            "-U",
            "axon",
            "-d",
            "postgres",
            "-X", // ignore ~/.psqlrc for deterministic behaviour
            "-q",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // psql unavailable (e.g. unit-only environment without PG): the sweep
        // is best-effort, so a missing binary must not fail the test run.
        Err(_) => return,
    };

    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        let _ = stdin.write_all(script.as_bytes());
    }
    let _ = child.wait();
}

/// Run [`sweep_stale_test_databases`] at most once per test process.
fn sweep_once(pg_port: &str) {
    static SWEEP: OnceLock<()> = OnceLock::new();
    SWEEP.get_or_init(|| {
        sweep_stale_test_databases(pg_port);
    });
}

/// REQ-AXO-91560 — guarantee `axon_test_template` carries the canonical
/// schema **and** the global SOLL seed before any test clones it.
///
/// The ephemeral-DB isolation (`createdb -T template`) hands each test a
/// pristine database, but a bare/empty template strips the ambient global
/// seed (the `PRO` sentinel rows + `GUI-PRO-*` guidelines) that the shared
/// devenv PG used to provide for free. Applying the idempotent
/// `db/ddl/*.sql` + `db/seed/*.sql` to the template once per process bakes the
/// seed INTO it, so every clone inherits the canonical baseline for free.
/// Reproducible on a fresh machine — no manual template setup required.
///
/// Runs at most once per process via `OnceLock`; `get_or_init` blocks
/// concurrent callers until the template is fully built, so no clone ever
/// sees a half-seeded template. Every psql command is synchronous and its
/// connection is closed before the first `createdb -T`, so the
/// "template in use" hazard cannot arise.
pub(crate) fn ensure_template_once(pg_port: &str) {
    static TEMPLATE: OnceLock<()> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let template = template_name();

        // Create the template database if absent. A pre-existing (possibly
        // empty) template is fine — the idempotent DDL+seed below brings it
        // to canonical state. A failure here (already exists) is ignored.
        let _ = std::process::Command::new("createdb")
            .args(["-h", "127.0.0.1", "-p", pg_port, "-U", "axon", &template])
            .output();

        // Apply canonical DDL then seed in lexical order, mirroring
        // scripts/lib/ensure-runtime.sh {apply_canonical_ddl,
        // apply_canonical_seed}. `generate_global_schema()` compiles the
        // same db/ddl files (DEC-AXO-082), so there is no schema divergence.
        let db_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("db");
        apply_sql_dir(pg_port, &template, &db_dir.join("ddl"));
        apply_sql_dir(pg_port, &template, &db_dir.join("seed"));
        apply_test_autoseed_triggers(pg_port, &template);
    });
}

/// REQ-AXO-91560 / REQ-AXO-901721 — install BEFORE INSERT auto-seed triggers
/// on the IST/SOLL tables **in the test template only**, so raw-SQL and
/// builder fixtures that insert `Symbol` / `Chunk` / `Edge` /
/// `GraphProjectionState` / `soll.Node` rows no longer have to hand-seed the
/// FK parents (`ist.Project`, `ist.IndexedFile`) or repeat `project_code` that
/// production guarantees via the A3 writer (REQ-AXO-901860 made
/// `project_code` a NOT NULL FK and `Chunk.file_path` a FK to `IndexedFile`).
/// Production DDL is untouched: the triggers live solely in
/// `axon_test_template`, and every `createdb -T` clone inherits them.
/// Idempotent (`CREATE OR REPLACE` + `DROP TRIGGER IF EXISTS`).
///
/// This is the root-cause fix for the whole class of `Writer Error: INSERT
/// INTO ist.* ... FK` test failures: a trigger covers every insert site —
/// present and future — with zero per-test boilerplate.
fn apply_test_autoseed_triggers(pg_port: &str, dbname: &str) {
    const SQL: &str = "\
CREATE OR REPLACE FUNCTION ist.test_autoseed_project() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    INSERT INTO ist.Project (code) VALUES (NEW.project_code) ON CONFLICT (code) DO NOTHING;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autoseed_chunk() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    INSERT INTO ist.Project (code) VALUES (NEW.project_code) ON CONFLICT (code) DO NOTHING;\n\
    IF NEW.file_path IS NOT NULL THEN\n\
        INSERT INTO ist.IndexedFile (path, project_code, last_seen_ms)\n\
        VALUES (NEW.file_path, NEW.project_code, 0) ON CONFLICT (path) DO NOTHING;\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autoseed_gps() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := upper(split_part(NEW.anchor_id, '::', 1));\n\
    END IF;\n\
    INSERT INTO ist.Project (code) VALUES (NEW.project_code) ON CONFLICT (code) DO NOTHING;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_symbol ON ist.Symbol;\n\
CREATE TRIGGER trg_test_autoseed_symbol BEFORE INSERT ON ist.Symbol\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_project();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_edge ON ist.Edge;\n\
CREATE TRIGGER trg_test_autoseed_edge BEFORE INSERT ON ist.Edge\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_project();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_chunk ON ist.Chunk;\n\
CREATE TRIGGER trg_test_autoseed_chunk BEFORE INSERT ON ist.Chunk\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_chunk();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_gps ON ist.GraphProjectionState;\n\
CREATE TRIGGER trg_test_autoseed_gps BEFORE INSERT ON ist.GraphProjectionState\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_gps();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_gembed ON ist.GraphEmbedding;\n\
CREATE TRIGGER trg_test_autoseed_gembed BEFORE INSERT ON ist.GraphEmbedding\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_gps();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_gproj ON ist.GraphProjection;\n\
CREATE TRIGGER trg_test_autoseed_gproj BEFORE INSERT ON ist.GraphProjection\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_gps();\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_revision() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.revision_id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_revpreview() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.preview_id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_revision ON soll.Revision;\n\
CREATE TRIGGER a_test_autofill_soll_revision BEFORE INSERT ON soll.Revision\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_revision();\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_revchange ON soll.RevisionChange;\n\
CREATE TRIGGER a_test_autofill_soll_revchange BEFORE INSERT ON soll.RevisionChange\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_revision();\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_revpreview ON soll.RevisionPreview;\n\
CREATE TRIGGER a_test_autofill_soll_revpreview BEFORE INSERT ON soll.RevisionPreview\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_revpreview();\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_node() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_edge() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.source_id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_node ON soll.Node;\n\
CREATE TRIGGER a_test_autofill_soll_node BEFORE INSERT ON soll.Node\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_node();\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_edge ON soll.Edge;\n\
CREATE TRIGGER a_test_autofill_soll_edge BEFORE INSERT ON soll.Edge\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_edge();\n";

    let mut child = match std::process::Command::new("psql")
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            pg_port,
            "-U",
            "axon",
            "-d",
            dbname,
            "-X",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return,
    };
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        let _ = stdin.write_all(SQL.as_bytes());
    }
    let _ = child.wait();
}

/// Apply every `NN_*.sql` file in `dir` (lexical order) to `dbname` via
/// psql. Best-effort: a missing directory or psql binary is a silent no-op
/// (unit-only environments without PG), matching the sweep's tolerance.
fn apply_sql_dir(pg_port: &str, dbname: &str, dir: &Path) {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "sql")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.bytes().next())
                        .is_some_and(|b| b.is_ascii_digit())
            })
            .collect(),
        Err(_) => return,
    };
    files.sort();
    for f in files {
        let Some(path) = f.to_str() else { continue };
        let _ = std::process::Command::new("psql")
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                pg_port,
                "-U",
                "axon",
                "-d",
                dbname,
                "-X",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "-f",
                path,
            ])
            .output();
    }
}
