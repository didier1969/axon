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

## Sub-Agent Policy (MANDATORY)
- **NEVER** spawn sub-agents (Agent tool) for code exploration, symbol lookup, architecture audit, or codebase understanding. Sub-agents cannot access MCP tools — they fall back to raw file reads, consuming 100-200K tokens to reconstruct what Axon IST already knows.
- **USE Axon MCP** from the main thread for all code intelligence: `query` → `inspect` → `retrieve_context` → `impact` → `anomalies` → `architectural_drift`.
- **Sub-agents are ONLY permitted for:** shell command execution (`cargo build/test`), document writing (no source reading), and tasks explicitly independent of codebase understanding.
- **SOLL tools** (`soll_manager`, `soll_work_plan`, `soll_query_context`) must be used for all project planning and documentation — never create standalone markdown plans.

## Runtime — 4-verb canonical surface (DEC-AXO-060)
Daily ops use exactly **1 entrypoint + 2 aliases + 4 verbs**:
- Entrypoint: `./scripts/axon [--instance live|dev] <verb>`
- Aliases: `./scripts/axon-live <verb>` / `./scripts/axon-dev <verb>`
- Verbs: `start`, `stop`, `status`, `qualify`

Examples:
- `./scripts/axon-dev start --indexer-full` — start dev with vectorization
- `./scripts/axon-live status` — check live runtime
- `./scripts/axon-live stop --hard` — stop live, force teardown
- `./scripts/axon qualify --profile smoke --mode graph_only` — runtime qualification (defaults to dev)
- `./scripts/axon qualify-mcp --surface core --checks quality,latency` — MCP-surface qualification

Operator skill (full reference): `docs/skills/axon-engineering-protocol/SKILL.md`.

## Data Policy
- **SOLL:** NEVER delete. Intentional truth (visions, requirements, decisions). Use `soll_rollback_revision` if needed.
- **IST (dev):** Delete freely. Rebuilt by indexer from source files.
- **IST (live):** Delete ONLY on explicit user request. Serves MCP clients.

## Deployment Pipeline (MANDATORY)
- **NEVER** manually `cargo build --release` + copy binaries to `bin/`. Always use the promotion pipeline.
- **Dev → Live promotion:** `bash scripts/release/promote_live_safe.sh --project AXO`
- **Rollback:** `bash scripts/release/rollback_live.sh`
- **Dev builds:** `cargo build` (debug, to `.axon/cargo-target/debug/`)
- **Live binaries:** Installed by `promote_live.sh` to `bin/` (release builds)
