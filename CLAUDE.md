# Axon: Structural Intelligence MCP Server

## Quick Start
`project_code` auto-resolved from cwd.
1. `help()` — identity, routing, schemas
2. `status()` — runtime truth, project, next action
3. `query("symbol")` — find code symbols
4. `help(tool=X)` — tool schema + examples

## Build & Test
```
cargo build --manifest-path src/axon-core/Cargo.toml --release
cargo test  --manifest-path src/axon-core/Cargo.toml --lib
cargo test  --manifest-path src/axon-core/Cargo.toml --bins
```
Binaries: `axon-brain` (MCP) · `axon-indexer` (IST writer) · `axonctl` (supervisor).

## Architecture
| Component | Path | Note |
|---|---|---|
| Runtime | `src/axon-core/` | Rust |
| DB | embedded DuckDB | canonical IST + SOLL |
| GPU | `src/axon-core/src/embedder/` | ONNX Runtime, CUDA/TensorRT EP, subprocess IPC |
| MCP server | `src/axon-core/src/mcp/` | 60 public tools |
| Visualization | Memgraph | human-only, non-canonical |
| Dashboard | Elixir/Phoenix | observation only |
| Supervisor | `src/axon-core/src/bin/axonctl.rs` | — |

## Tool Routing
| Task | Tool |
|---|---|
| Find symbol | `query` |
| Inspect detail | `inspect` |
| Evidence packet | `retrieve_context` |
| Blast radius | `impact` |
| Why it exists | `why` |
| Dependency flow | `path` |
| Structural risks | `anomalies` |
| SOLL intent | `soll_query_context` |
| Commit work | `axon_pre_flight_check` → `axon_commit_work` |

## Sub-Agent Policy
- Forbidden for code exploration / symbol lookup / arch audit / codebase understanding (no MCP → 100-200K tokens wasted reconstructing IST).
- Use Axon MCP from main thread: `query` → `inspect` → `retrieve_context` → `impact` → `anomalies` → `architectural_drift`.
- Allowed only: shell exec (`cargo build/test`), doc writing (no source reading), MCP-independent tasks.
- Planning/docs → SOLL tools (`soll_manager`, `soll_work_plan`, `soll_query_context`). Never standalone markdown plans.

## Runtime — 4-verb canonical (DEC-AXO-060)
Surface: `./scripts/axon [--instance live|dev] {start|stop|status|qualify}`
Aliases: `./scripts/axon-live` · `./scripts/axon-dev`
```
./scripts/axon-dev start --indexer-full          # dev + vectorization
./scripts/axon-live status
./scripts/axon-live stop --hard                  # force teardown
./scripts/axon qualify --profile smoke --mode graph_only  # defaults to dev
./scripts/axon qualify-mcp --surface core --checks quality,latency
```
Full operator reference: `docs/skills/axon-engineering-protocol/SKILL.md`.

## Data Policy
- SOLL: NEVER delete (visions/requirements/decisions). Roll back via `soll_rollback_revision`.
- IST dev: delete freely; rebuilt by indexer from source.
- IST live: delete only on explicit user request; serves MCP clients.

## Deployment Pipeline
- NEVER manual `cargo build --release` + copy to `bin/`. Use the pipeline.
- Promote dev→live: `bash scripts/release/promote_live_safe.sh --project AXO`
- Rollback: `bash scripts/release/rollback_live.sh`
- Dev builds: `cargo build` (debug → `.axon/cargo-target/debug/`).
- Live binaries: installed by `promote_live.sh` to `bin/` (release).
