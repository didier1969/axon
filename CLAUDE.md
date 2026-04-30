# Axon: Structural Intelligence MCP Server

## Quick Start
`project_code` is auto-resolved from your working directory. No manual discovery needed.

1. `help()` — Axon identity, tool routing, input schemas
2. `status()` — runtime truth, auto-detected project, next action
3. `query("symbol_name")` — find code symbols
4. `help(tool=X)` — any tool's JSON input schema and examples

## Build & Test
- Build: `cargo build --manifest-path src/axon-core/Cargo.toml --release`
- Test: `cargo test --manifest-path src/axon-core/Cargo.toml --lib`
- Test bins: `cargo test --manifest-path src/axon-core/Cargo.toml --bins`
- Binaries: `axon-brain` (MCP), `axon-indexer` (IST writer), `axonctl` (supervisor)

## Architecture
- **Runtime:** Rust (`src/axon-core/`)
- **Database:** DuckDB (embedded, canonical IST + SOLL)
- **GPU:** ONNX Runtime with CUDA/TensorRT EP, subprocess IPC (`src/axon-core/src/embedder/`)
- **MCP Server:** `src/axon-core/src/mcp/` — 60 public tools
- **Visualization:** Memgraph (human-only, non-canonical)
- **Dashboard:** Elixir/Phoenix (observation only)
- **Supervisor:** `src/axon-core/src/bin/axonctl.rs`

## Key Tool Routing
| Task | Tool |
|------|------|
| Find symbol | `query` |
| Inspect detail | `inspect` |
| Evidence packet | `retrieve_context` |
| Blast radius | `impact` |
| Why it exists | `why` |
| Dependency flow | `path` |
| Structural risks | `anomalies` |
| SOLL intent | `soll_query_context` |
| Commit work | `axon_pre_flight_check` → `axon_commit_work` |

## Runtime
- **Operator skill:** `docs/skills/axon-engineering-protocol/SKILL.md`
- **Start:** `./scripts/axon --instance dev start` / `./scripts/axon --instance live start`
- **Stop:** `./scripts/axon --instance dev stop`
- **Qualify:** `./scripts/axon qualify-mcp`
