# Axon: Structural Intelligence MCP Server

## Quick Start
`project_code` auto-resolved from cwd.
1. `help()` — identity, routing, schemas
2. `status()` — runtime truth, project, next action
3. `query("symbol")` — find code symbols
4. `help(tool=X)` — tool schema + examples

## IST reads — usable by default (freshness = trust calibration, NOT a gate)
`query`/`inspect`/`impact`/`why`/`anomalies`/`path` are usable whenever `status` returns — **including when the indexer is idle**. `status` leads with `IST reads: usable` + the real lag `modified_files_since`: **0 = snapshot current**; **N>0 = cross-check those N files before high-stakes mutations**. Start indexer-graph only for continuous live refresh — never decline structural tools on a process-liveness flag. Genuine unavailability surfaces as an explicit `Blocker`. The historical bias: LLMs saw `stale`/`degraded`/`blocker` and fell back to grep (REQ-AXO-901871 / REQ-AXO-087 family). Contract: **CPT-AXO-029**.

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
| DB | PostgreSQL 17 + pgvector | canonical IST + SOLL. AGE retired (MIL-AXO-017, REQ-AXO-90005). DuckDB fully purged (REQ-AXO-271 slices 2-6). IST edges canonical = RAM IstGraphView (CSR snapshot, PIL-AXO-9002) ; PG `public.edge` is the persistence layer + fallback when RAM cold. |
| Streaming pipeline v2 | `src/axon-core/src/pipeline/` | A1/A2/A3 (graph + chunks + FTS, CPU) → try_send → B1/B2/B3 (GPU embed). REQ-AXO-289 / CPT-AXO-054 (session 19 canonical). Diagram: `docs/architecture/visualize-nexus-pull.html`. |
| GPU | `src/axon-core/src/embedder/` | ONNX Runtime, CUDA/TensorRT EP, BGE-Large 1024d. `GpuB2Embedder` (pipeline/embedder_gpu.rs) is the v2 wrapper. |
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
| **Structural Health Index** (aggregate, RAM-native) | `structural_health_index` (CPT-AXO-90055 ; supersedes `health` ; needs `ist_snapshot_warm`) |
| **Remediation worklist** (ranked debt targets) | `structural_health_worklist` (untested hubs + worst-coupled modules) |
| **Dead-cluster detection** (mutually-wired but globally unreachable) | `orphan_clusters` (REQ-AXO-902211 ; complements `wiring`'s per-symbol check, which misses clusters that only call each other) |
| SOLL intent | `soll_query_context` |
| Commit work | `axon_pre_flight_check` → `axon_commit_work` |
| Hybrid retrieval (FTS+vector+graph) | `retrieve_context` / `retrieve_context_layered` |
| Toggle query-embed provider (runtime) | `embed_provider` |
| **Recall how-to-work memory** (PRIMARY, at init) | `practice_recall` |
| **Save a learned practice** (governed, decaying) | `practice_put` (scope/role/model partitioning, REQ-AXO-902149) |
| **Reinforce/consolidate practices** | `practice_tick` / `practice_card` |
| **Read this project's inbox** (wake/handoff) | `mcp_inbox_read` |
| **Send to another project** | `mcp_outbox_send` |

> **New governed tools — USE THEM (don't fall back to files).** `practice_*` (REQ-AXO-902131+) is the PRIMARY "how to work" memory channel: `practice_recall` at init, `practice_put` for every durable lesson. `mcp_inbox_read`/`mcp_outbox_send` (REQ-AXO-902114+) are the cross-project mailbox. If your client registry predates them they may be missing — reconnect MCP to refresh (`mcp_surface_diagnostics` confirms server vs client). `feedback_*.md` + `MEMORY.md` are FALLBACK only.

## Query-embed provider (REQ-AXO-901978/901984)
Query/`why`/`retrieve_context` embed the NL question. `start.sh` provisions the GPU ORT artifact whenever a GPU is detected — **including `brain_only`** — so the punctual query embed runs on the idle GPU (~ms vs ~s on CPU). `query`/`retrieve_context`/`_layered` accept `semantic=auto|lexical|semantic` (auto = single-token symbol → lexical/no-embed, NL → embed). Toggle the provider at RUNTIME without a restart via `embed_provider` (action=set, provider=cpu|gpu|auto) — use `cpu` to release the GPU for Axon Live / another service, `gpu` to re-grab it. `status`/`embedding_status` report the true worker compute (GPU/CPU).

## Pipeline v2 bench
```
# Env REQUIS pour --gpu (sinon CUDA error 35 sur WSL2 — voir feedback_bench_gpu_ld_library_path) :
export ORT_STRATEGY=system
export ORT_DYLIB_PATH=$(jq -r .core_lib .axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json)
export LD_LIBRARY_PATH=/usr/lib/wsl/lib:$(dirname $ORT_DYLIB_PATH):${LD_LIBRARY_PATH:-}
export AXON_DEV_DATABASE_URL=postgres://axon@127.0.0.1:44144/axon_dev

cargo run --manifest-path src/axon-core/Cargo.toml --release --bin axon-bench-pipeline-v2 -- --source <PATH> --max-files N --gpu --human
```
Modes: `--gpu` (production), `--cpu` (ORT CPU EP), `--noop` (smoke, no GPU/PG). CSV output via `--csv`. Reports per-stage `items_in/out/err/bp` + Symbol/Chunk/IndexedFile/ChunkEmbedding row counts via writer ctx (reader ctx is stale during the shutdown window on the embedded test backend). See REQ-AXO-289 / CPT-AXO-054 for the topology.

## Sub-Agent Policy (GUI-PRO-027)
- Sub-agents reach Axon MCP **first-class** (pass `project="AXO"` explicit; tools resolve via ToolSearch). Use them for parallel RCA / research / MCP reads. Caveat: each costs ~10-30K tokens, so spawn deliberately.
- Rust **edits and builds stay serial orchestrator-side** (cargo = global lock); never run concurrent compiled-core builds across agents.
- Never delegate: SOLL mutation, promote-live, or any destructive op.
- Main-thread default for code nav: `query` → `inspect` → `retrieve_context` → `impact` → `anomalies` → `architectural_drift`.
- Planning/docs → SOLL tools (`soll_manager`, `soll_work_plan`, `soll_query_context`). Never standalone markdown plans.

## Runtime — 4-verb canonical (DEC-AXO-060)
Surface: `./scripts/axon [--instance live|dev] {start|stop|status|qualify}`
Aliases: `./scripts/axon-live` · `./scripts/axon-dev`
```
./scripts/axon-dev start full          # dev + vectorization
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
Procedure = **GUI-PRO-028** in SOLL (6 steps : practice_put apprentissages / session_pointer / SOLL cleanup+replan / boot-docs prune / SKILL consolidation / working-notes + handoff mailbox). Trigger : "Axon Hand Off" / "handoff" / `/clear` imminent / reprise par un autre LLM — **PAS sur le % de contexte** (unlimited-context). Mémoire « comment travailler » = `practice_*` PRIMAIRE (REQ-AXO-902131), `feedback_*.md` = fallback gracieux si registre client stale. Read body via `soll_query_context` or `retrieve_context question="GUI-PRO-028 body"`. NO content duplicated here.

## Deployment Pipeline
- NEVER manual `cargo build --release` + copy to `bin/`. Use the pipeline.
- Promote dev→live: `bash scripts/release/promote_live_safe.sh --project AXO`
- Rollback: `bash scripts/release/rollback_live.sh`
- Dev builds: `cargo build` (debug → `.axon/cargo-target/debug/`).
- Live binaries: installed by `promote_live.sh` to `bin/` (release).
