# Core Refactor Candidates After `mcp.rs`

Date: 2026-03-30
Status: proposed
Scope: `src/axon-core/src/graph.rs` and `src/axon-core/src/main.rs`

## 1. `graph.rs`

Path:

- `/home/dstadel/projects/axon/src/axon-core/src/graph.rs`

Current size:

- 587 lines

### Why It Needs Refactoring

`graph.rs` currently mixes too many layers:

- plugin discovery
- FFI bootstrap
- session setup
- schema initialization
- raw SQL execution
- parameter expansion
- ingestion writes
- pending-file claiming
- embedding updates
- graph-derived analytics

That makes it hard to change one axis without touching the rest.

### Current Responsibility Groups

1. FFI and low-level DB lifecycle
- `LatticePool`
- `Drop for LatticePool`
- function pointers and library loading

2. Database bootstrap and session wiring
- `GraphStore::new`
- `setup_session`
- `find_plugin_path`
- `init_schema`

3. Query and execution primitives
- `execute`
- `execute_param`
- `query_json`
- `query_json_param`
- `query_count`
- `query_count_param`
- `query_on_ctx`
- `expand_named_params`
- `execute_batch`

4. Ingestion persistence
- `bulk_insert_files`
- `insert_file_data_batch`
- `fetch_pending_batch`
- `fetch_unembedded_symbols`
- `update_symbol_embeddings`

5. Analytics and governance helpers
- `get_security_audit`
- `get_coverage_score`
- `get_technical_debt`
- `get_god_objects`

### Recommended Target Shape

- `graph/mod.rs`
  - `GraphStore`
  - high-level public API
- `graph/ffi.rs`
  - FFI types
  - `LatticePool`
  - library loading
- `graph/bootstrap.rs`
  - plugin discovery
  - DB bootstrap
  - session attach/setup
  - schema creation
- `graph/query.rs`
  - execute/query primitives
  - parameter expansion
- `graph/ingestion.rs`
  - file insertion
  - pending claim
  - symbol embedding updates
- `graph/analytics.rs`
  - audit, coverage, debt, god-object helpers

### Best Extraction Order

1. `analytics.rs`
- easiest to move with low coupling
- good first reduction of file size

2. `query.rs`
- central but mechanically separable
- clarifies the real storage API

3. `ingestion.rs`
- isolates the persistence workflow used by workers and scanner

4. `bootstrap.rs`
- higher risk because it touches startup and DB attachment
- do only after tests are already green and stable

5. `ffi.rs`
- safest last, once module boundaries are settled

### Main Risks

- breaking bootstrap of `ist.db` / `soll.db`
- breaking plugin resolution
- changing transactional semantics of `fetch_pending_batch`
- scattering the public storage API too aggressively

### Recommendation

Start with `analytics.rs`, then `query.rs`.

That reduces file size fast without destabilizing startup.

## 2. `main.rs`

Path:

- `/home/dstadel/projects/axon/src/axon-core/src/main.rs`

Current size:

- 302 lines

### Why It Needs Refactoring

`main.rs` is smaller than `graph.rs`, but structurally denser.

Almost the entire runtime is embedded in one `main()`:

- Tokio runtime construction
- environment resolution
- DB initialization
- queue setup
- worker setup
- embedder startup
- MCP HTTP startup
- autonomous ingestor loop
- initial scanner thread
- telemetry socket binding
- telemetry command parsing
- watchdog thread

This is not primarily a line-count problem. It is a control-flow concentration problem.

### Current Responsibility Groups

1. Boot configuration
- env resolution
- paths
- tracing init
- runtime limits

2. Service startup
- graph store
- queue
- writer actor
- worker pool
- semantic worker
- MCP HTTP server

3. Background loops
- memory watchdog
- autonomous ingestor
- initial auto-scan

4. Telemetry transport
- Unix socket binding
- outgoing broadcast loop
- incoming command loop

5. Telemetry command protocol
- `EXECUTE_CYPHER`
- `RAW_QUERY`
- `SESSION_INIT`
- `PARSE_BATCH`
- `PULL_PENDING`
- `SCAN_ALL`
- `RESET`

### Recommended Target Shape

- `main.rs`
  - thin startup entrypoint only
- `runtime/bootstrap.rs`
  - tracing
  - env/path resolution
  - shared boot config
- `runtime/services.rs`
  - graph store
  - queue
  - worker pool
  - semantic worker
  - MCP HTTP
- `runtime/background.rs`
  - watchdog
  - autonomous ingestor
  - initial scanner
- `runtime/telemetry.rs`
  - socket listener
  - connection handling
  - outgoing broadcast loop
- `runtime/telemetry_protocol.rs`
  - parse and execute textual commands
  - `PARSE_BATCH`
  - `PULL_PENDING`
  - `SCAN_ALL`
  - session init

### Best Extraction Order

1. `telemetry_protocol.rs`
- the cleanest seam
- highest readability gain
- keeps behavior local and testable

2. `background.rs`
- next clean seam
- removes loop noise from `main()`

3. `services.rs`
- clarifies startup responsibilities

4. `bootstrap.rs`
- small but useful final polish

### Main Risks

- changing ordering during startup
- breaking ownership and cloning patterns across tasks
- accidentally changing socket behavior or broadcast semantics

### Recommendation

Start with `telemetry_protocol.rs`.

It is the highest leverage extraction because:

- it concentrates operator-facing commands
- it is the most likely place to grow further
- it directly affects validation en conditions reelles

## Priority Order After `mcp.rs`

1. `graph.rs`
2. `main.rs`
3. `pool_facade.ex`
4. `server.ex`

Why:

- Rust core modules still carry the heaviest structural coupling
- Elixir becomes easier to trim once the Rust contracts are cleaner
