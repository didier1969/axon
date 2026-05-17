# Axon: Structural Intelligence MCP Server

## Quick Start
`project_code` auto-resolved from cwd.
1. `help()` — identity, routing, schemas
2. `status()` — runtime truth, project, next action
3. `query("symbol")` — find code symbols
4. `help(tool=X)` — tool schema + examples

## ⚠️ IST freshness gate
Before relying on `inspect`/`query`/`impact`, verify `status` reports `freshness: fresh` + `trust: canonical`. If degraded, brain serves a stale snapshot — start indexer-graph alongside. Full contract + recovery commands: **CPT-AXO-029**.

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
| DB | PostgreSQL 17 + pgvector | canonical IST + SOLL. AGE retired (MIL-AXO-017 delivered, REQ-AXO-90005). DuckDB purged at user-facing level (REQ-AXO-271 cleanup track remains for residual dual-backend abstractions). IST edges in `public.Edge` + `WITH RECURSIVE` SQL functions (`db/ddl/04_graph_functions.sql`). |
| Streaming pipeline v2 | `src/axon-core/src/pipeline_v2/` | A1/A2/A3 (graph + chunks + FTS, CPU) → try_send → B1/B2/B3 (GPU embed). REQ-AXO-289 / CPT-AXO-054 (session 19 canonical). Diagram: `docs/architecture/visualize-nexus-pull.html`. |
| GPU | `src/axon-core/src/embedder/` | ONNX Runtime, CUDA/TensorRT EP, BGE-Large 1024d. `GpuB2Embedder` (pipeline_v2/embedder_gpu.rs) is the v2 wrapper. |
| MCP server | `src/axon-core/src/mcp/` | live tool count via `status mode=brief` |
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
| Hybrid retrieval (FTS+vector+graph RRF) | `retrieve_context_v2` (MIL-AXO-017 slice 4 / REQ-AXO-298) |
| Lexical / text search | `code_search` (REQ-AXO-292 backlog, largely subsumed by `retrieve_context_v2`) |

## Pipeline v2 bench
```
cargo run --release --bin axon-bench-pipeline-v2 -- --source <PATH> --max-files N --gpu --human
```
Modes: `--gpu` (production), `--cpu` (ORT CPU EP), `--noop` (smoke, no GPU/PG). CSV output via `--csv`. Reports per-stage `items_in/out/err/bp` + Symbol/Chunk/IndexedFile/ChunkEmbedding row counts via writer ctx (reader ctx is stale during the shutdown window on the embedded test backend). See REQ-AXO-289 / CPT-AXO-054 for the topology.

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

## Session Hand Off (every session end)
Procedure = **GUI-PRO-028** in SOLL (5 mandatory steps : session_pointer / SOLL cleanup+replan / boot-docs prune / SKILL.md consolidation / working-notes audit). Trigger : "Axon Hand Off" or context approaching 70%. Read body via `soll_query_context` or `retrieve_context question="GUI-PRO-028 body"`. NO content duplicated here.

## Deployment Pipeline
- NEVER manual `cargo build --release` + copy to `bin/`. Use the pipeline.
- Promote dev→live: `bash scripts/release/promote_live_safe.sh --project AXO`
- Rollback: `bash scripts/release/rollback_live.sh`
- Dev builds: `cargo build` (debug → `.axon/cargo-target/debug/`).
- Live binaries: installed by `promote_live.sh` to `bin/` (release).
