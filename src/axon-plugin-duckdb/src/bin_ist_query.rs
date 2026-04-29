// Copyright (c) Didier Stadelmann. All rights reserved.

use duckdb::types::Value as DuckValue;
use duckdb::{AccessMode, Config, Connection, Result as DuckResult, Row};
use serde_json::{json, Map, Value as JsonValue};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Csv,
    Tsv,
}

impl OutputFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "csv" => Some(Self::Csv),
            "tsv" | "table" => Some(Self::Tsv),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Args {
    db_path: PathBuf,
    format: OutputFormat,
    sql: String,
}

fn usage() -> String {
    [
        "usage: ist-query [--db PATH] [--format json|csv|tsv] [--preset NAME] <SQL>",
        "",
        "presets:",
        "  recent-vector-batches",
        "  slow-vector-batches",
        "  vector-rollup-hour",
    ]
    .join("\n")
}

fn default_db_path() -> PathBuf {
    PathBuf::from(".axon/graph_v2/ist.db")
}

fn preset_sql(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "recent-vector-batches" => Some(
            "SELECT run_id, \
                    finished_at_ms - started_at_ms AS wall_ms, \
                    chunk_count, \
                    file_count, \
                    input_bytes, \
                    fetch_ms, \
                    embed_ms, \
                    db_write_ms, \
                    mark_done_ms, \
                    success \
             FROM VectorBatchRun \
             ORDER BY finished_at_ms DESC \
             LIMIT 50",
        ),
        "slow-vector-batches" => Some(
            "SELECT run_id, \
                    finished_at_ms - started_at_ms AS wall_ms, \
                    chunk_count, \
                    file_count, \
                    input_bytes, \
                    fetch_ms, \
                    embed_ms, \
                    db_write_ms, \
                    mark_done_ms, \
                    success \
             FROM VectorBatchRun \
             WHERE success = true \
             ORDER BY embed_ms DESC, wall_ms DESC \
             LIMIT 50",
        ),
        "vector-rollup-hour" => Some(
            "SELECT bucket_start_ms, \
                    project_code, \
                    model_id, \
                    chunks_embedded, \
                    files_vector_ready, \
                    batches, \
                    fetch_ms_total, \
                    embed_ms_total, \
                    db_write_ms_total, \
                    mark_done_ms_total \
             FROM HourlyVectorizationRollup \
             ORDER BY bucket_start_ms DESC \
             LIMIT 24",
        ),
        _ => None,
    }
}

fn parse_args() -> Result<Args, String> {
    let mut db_path = default_db_path();
    let mut format = OutputFormat::Json;
    let mut sql: Option<String> = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db" => {
                let Some(path) = args.next() else {
                    return Err("--db requires a path".to_string());
                };
                db_path = PathBuf::from(path);
            }
            "--format" => {
                let Some(raw) = args.next() else {
                    return Err("--format requires a value".to_string());
                };
                format = OutputFormat::parse(&raw)
                    .ok_or_else(|| format!("unsupported format: {raw}"))?;
            }
            "--preset" => {
                let Some(name) = args.next() else {
                    return Err("--preset requires a value".to_string());
                };
                sql = Some(
                    preset_sql(&name)
                        .ok_or_else(|| format!("unknown preset: {name}"))?
                        .to_string(),
                );
            }
            "--help" | "-h" => {
                return Err(usage());
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option: {other}"));
            }
            other => {
                let mut parts = vec![other.to_string()];
                parts.extend(args);
                sql = Some(parts.join(" "));
                break;
            }
        }
    }

    let Some(sql) = sql else {
        return Err(usage());
    };

    Ok(Args {
        db_path,
        format,
        sql,
    })
}

fn open_read_only(path: &PathBuf) -> Result<Connection, String> {
    let config = Config::default()
        .access_mode(AccessMode::ReadOnly)
        .map_err(|err| format!("failed to configure read-only DuckDB access: {err}"))?;
    Connection::open_with_flags(path, config)
        .map_err(|err| format!("failed to open {}: {err}", path.display()))
}

fn row_to_json(row: &Row<'_>, columns: &[String]) -> DuckResult<JsonValue> {
    let mut object = Map::new();
    for (idx, column) in columns.iter().enumerate() {
        let value: DuckValue = row.get(idx)?;
        object.insert(column.clone(), value_to_json(value));
    }
    Ok(JsonValue::Object(object))
}

fn value_to_json(value: DuckValue) -> JsonValue {
    match value {
        DuckValue::Null => JsonValue::Null,
        DuckValue::Boolean(v) => json!(v),
        DuckValue::TinyInt(v) => json!(v),
        DuckValue::SmallInt(v) => json!(v),
        DuckValue::Int(v) => json!(v),
        DuckValue::BigInt(v) => json!(v),
        DuckValue::UTinyInt(v) => json!(v),
        DuckValue::USmallInt(v) => json!(v),
        DuckValue::UInt(v) => json!(v),
        DuckValue::UBigInt(v) => json!(v),
        DuckValue::Float(v) => json!(v),
        DuckValue::Double(v) => json!(v),
        DuckValue::Decimal(v) => json!(v.to_string()),
        DuckValue::Text(v) => json!(v),
        DuckValue::Blob(v) => json!(String::from_utf8_lossy(&v).to_string()),
        DuckValue::Date32(v) => json!(v.to_string()),
        DuckValue::Time64(_, v) => json!(v.to_string()),
        DuckValue::Timestamp(_, v) => json!(v.to_string()),
        DuckValue::Interval {
            months,
            days,
            nanos,
        } => json!({
            "months": months,
            "days": days,
            "nanos": nanos,
        }),
        DuckValue::HugeInt(v) => json!(v.to_string()),
        DuckValue::Enum(v) => json!(v),
        DuckValue::List(values) | DuckValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(value_to_json).collect())
        }
        DuckValue::Struct(entries) => {
            let mut object = Map::new();
            for (key, value) in entries.iter() {
                object.insert(key.clone(), value_to_json(value.clone()));
            }
            JsonValue::Object(object)
        }
        DuckValue::Map(entries) => {
            let mut object = Map::new();
            for (key, value) in entries.iter() {
                object.insert(
                    format!("{:?}", value_to_json(key.clone())),
                    value_to_json(value.clone()),
                );
            }
            JsonValue::Object(object)
        }
        DuckValue::Union(value) => value_to_json(*value),
    }
}

fn value_to_cell(value: &JsonValue, delimiter: char) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::String(v) => escape_delimited(v, delimiter),
        other => escape_delimited(&other.to_string(), delimiter),
    }
}

fn escape_delimited(raw: &str, delimiter: char) -> String {
    if delimiter == '\t' {
        return raw.replace('\t', " ").replace('\n', "\\n");
    }
    if raw.contains(delimiter) || raw.contains('\n') || raw.contains('"') {
        let escaped = raw.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        raw.to_string()
    }
}

fn print_delimited(rows: &[JsonValue], columns: &[String], delimiter: char) {
    let header = columns.join(&delimiter.to_string());
    println!("{header}");
    for row in rows {
        let JsonValue::Object(object) = row else {
            continue;
        };
        let cells = columns
            .iter()
            .map(|column| value_to_cell(object.get(column).unwrap_or(&JsonValue::Null), delimiter))
            .collect::<Vec<_>>();
        println!("{}", cells.join(&delimiter.to_string()));
    }
}

fn run(args: Args) -> Result<(), String> {
    let connection = open_read_only(&args.db_path)?;
    let mut stmt = connection
        .prepare(&args.sql)
        .map_err(|err| format!("failed to prepare query: {err}"))?;
    let mut mapped = stmt
        .query([])
        .map_err(|err| format!("failed to execute query: {err}"))?;
    let statement = mapped
        .as_ref()
        .ok_or_else(|| "query did not return statement metadata".to_string())?;
    let columns = (0..statement.column_count())
        .map(|idx| {
            statement
                .column_name(idx)
                .map(|name| name.to_string())
                .map_err(|err| format!("failed to read column {idx} name: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = Vec::new();
    while let Some(row) = mapped
        .next()
        .map_err(|err| format!("failed to read row: {err}"))?
    {
        rows.push(row_to_json(row, &columns).map_err(|err| format!("failed to read row: {err}"))?);
    }

    match args.format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "db_path": args.db_path.display().to_string(),
                    "columns": columns,
                    "rows": rows,
                }))
                .map_err(|err| format!("failed to render JSON: {err}"))?
            );
        }
        OutputFormat::Csv => print_delimited(&rows, &columns, ','),
        OutputFormat::Tsv => print_delimited(&rows, &columns, '\t'),
    }

    Ok(())
}

fn main() {
    let exit = match parse_args().and_then(run) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    };
    std::process::exit(exit);
}
