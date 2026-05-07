//! MIL-AXO-015 P5 slice 5c — SOLL bootstrap export tool.
//!
//! Reads a DuckDB `soll.db` (read-only) and emits a JSON snapshot
//! whose shape matches `axon_core::postgres::seed::SeedDocument`. The
//! operator runs this once per migration cycle to capture the live
//! SOLL state into a file the brain's PostgreSQL bootstrap can replay.
//!
//! Usage:
//!
//!     soll-export-seed <path-to-soll.db> [--output <out.json>]
//!
//! When `--output` is omitted, the JSON is written to stdout.
//!
//! Read-only access means this binary can be invoked safely while the
//! live brain still holds the database — `Config::access_mode(ReadOnly)`
//! coexists with the writer.

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

use anyhow::{anyhow, Context, Result};
use duckdb::{AccessMode, Config, Connection};

/// (json_key, duckdb_table_name) pairs, ordered to match
/// `SeedDocument` field order. The JSON keys are snake_case (Rust
/// convention); the table names follow the SOLL DDL (CamelCase).
const TABLES: &[(&str, &str)] = &[
    ("registry", "Registry"),
    ("nodes", "Node"),
    ("edges", "Edge"),
    ("revisions", "Revision"),
    ("revision_changes", "RevisionChange"),
    ("traceability", "Traceability"),
];

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: soll-export-seed <path-to-soll.db> [--output <file>]");
        std::process::exit(1);
    }
    let db_path = &args[1];

    let mut writer: Box<dyn Write> = match args.iter().position(|a| a == "--output") {
        Some(idx) => {
            let dest = args
                .get(idx + 1)
                .ok_or_else(|| anyhow!("--output requires a path"))?;
            Box::new(BufWriter::new(
                File::create(dest).with_context(|| format!("create output {dest}"))?,
            ))
        }
        None => Box::new(BufWriter::new(std::io::stdout().lock())),
    };

    let cfg = Config::default()
        .access_mode(AccessMode::ReadOnly)
        .map_err(|e| anyhow!("ReadOnly access mode rejected: {e}"))?;
    let conn = Connection::open_with_flags(db_path, cfg)
        .with_context(|| format!("open {db_path} read-only"))?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    writeln!(writer, "{{")?;
    writeln!(writer, "  \"version\": 1,")?;
    writeln!(writer, "  \"generated_at_ms\": {now_ms},")?;

    for (i, (json_key, table)) in TABLES.iter().enumerate() {
        // Force the schema name to `main` — DuckDB attaches the SOLL
        // database under that schema by default. `to_json(t)` emits one
        // JSON object per row keyed by column name (matching the Rust
        // SeedDocument field naming).
        let sql = format!("SELECT to_json(t) FROM main.{table} t");
        let rows = match conn.prepare(&sql) {
            Ok(mut stmt) => {
                let mapped = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .with_context(|| format!("query {table}"))?;
                let mut acc = Vec::new();
                for r in mapped {
                    acc.push(r.with_context(|| format!("row decode in {table}"))?);
                }
                acc
            }
            Err(e) => {
                eprintln!(
                    "warn: skipping {table} (prepare failed: {e}); writing empty array"
                );
                Vec::new()
            }
        };
        write!(writer, "  \"{json_key}\": [")?;
        for (j, row) in rows.iter().enumerate() {
            if j > 0 {
                write!(writer, ",")?;
            }
            // Each row is already a JSON object string from to_json.
            write!(writer, "{row}")?;
        }
        write!(writer, "]")?;
        if i + 1 < TABLES.len() {
            writeln!(writer, ",")?;
        } else {
            writeln!(writer)?;
        }
    }

    writeln!(writer, "}}")?;
    writer.flush()?;
    Ok(())
}
