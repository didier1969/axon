# Handoff — 2026-05-04 — Pipeline 2 perf tuning + LLM session memory architecture

> Session-private artifact for the next LLM/operator session.
> Canonical truth = SOLL entities. This file is chronological audit only.

## Part 1 — Live runtime state at handoff

- **Live build**: v0.8.0-167-g... (verify via `./scripts/axon-live status`)
- **Started**: 2026-05-04 ~17:51 UTC (post-WSL-crash recovery)
- **Role**: brain (no live indexer; verified pid 29122 holding SOLL guard)
- **MCP HTTP gateway**: `127.0.0.1:44129/mcp`
- **TensorRT engine cache**: `/home/dstadel/.cache/axon/fastembed/tensorrt/engine-cache/` —
  `axon-bge-large_4680136567211888080_0_fp16_sm86.engine` 680 MB, rebuilt 2026-05-04 07:42
- **Git**: main @ 9c333e9, pushed; tree clean except untracked CSV bench outputs and working notes
- **Dev**: stopped clean (no `.axon-dev/run-indexer/` pid)

## Part 2 — Shipped this session (SOLL canonical)

| ID | Title | Status | Commits |
|---|---|---|---|
| REQ-AXO-171 | Embedder subprocess silent crash | in_progress | 6cfdbbe |
| REQ-AXO-172 | AXON_WATCH_DIR ignored | complete | 3cea51a + 7c7351e |
| REQ-AXO-173 | ORT dylib dlopen fails | in_progress | caa6e6a |
| REQ-AXO-174 | axon-dev start ne rebuild pas debug binary | open | — |
| REQ-AXO-175 | probe.sh L3 harness | in_progress | d2cbbb9 |
| REQ-AXO-176 | Session memory auto-checkpoint cycle | open (proposal) | — |
| REQ-AXO-177 | L1 embedder bench harness | in_progress | 74bca8d, b8bfd88, 9c333e9 |
| DEC-AXO-067 | Operating discipline for LLM-driven sessions | accepted | — |

Live promotions this session: 5× to v0.8.0-167-g... (final).

## Part 3 — Pipeline 2 perf state

- **Single-worker BGE-Large TensorRT**: 152-155 chunks/s sustained (n=512-600, post-WSL-restart engine)
- **Target 30 chunks/s**: exceeded 5x
- **Stretch 200 chunks/s**: NOT validated yet
- **Multi-worker workers=2**: 34.37 chunks/s — collapsed (4.5× ratio matches 7GB VRAM RAM-spill pattern, validates DEC-AXO-067 rule 2)
- **ORT artifact**: TensorRT manifest at `.axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json` — `out_path` `/nix/store/0bk9hvccz0rhbrfjvx3628lqy3sgpyzm-onnxruntime-1.24.4`

## Part 4 — Open paths toward 200 chunks/s

| Path | Effort | Risk |
|---|---|---|
| Multi-worker w/ lower per-instance VRAM (smaller batch per worker or shared engine) | medium, requires `OrtGpuFirstTextEmbedding` refactor or stream usage | TensorRT recompile per shape |
| Larger n single-worker (n > 651) | requires extending bench source (embedder.rs caps at ~651 chunks) | low |
| INT8/FP8 quantization (BGE-Large has calibrated INT8) | new artifact build | medium |
| Investigate ~150 ms per-micro-batch TensorRT context switch | profiling, possibly NSight | low gain certainty |

## Part 5 — Operating discipline (DEC-AXO-067 — apply continuously)

1. **Axon Live `--graph-only`** when developing Axon itself; full live for other projects.
2. **NVML telemetry only** (`AXON_GPU_TELEMETRY_BACKEND=nvml`); ceiling 7 GB on 8 GB GPU.
3. **Cache mental SOLL IDs**; prefer `cypher` over `soll_query_context` when target ID is known.
4. **SOLL writes**: English, concise, structured, self-contained, precise.
5. **Stop at 70% context**: emit checkpoint via REQ-AXO-176 mechanism (when implemented).

## Part 6 — Known traps to avoid

- **WSL2 utility VM crash** on TensorRT first-compile under load (P9 protocol exception observed 2026-05-04 04:49 UTC). Pre-warm engines individually, low concurrent load.
- **`axon-dev start` does NOT rebuild debug binary** (REQ-AXO-174). Always pre-build via `CARGO_TARGET_DIR=.axon/cargo-target cargo build --bin axon-indexer` after Rust edits.
- **Background bash with `| tail -N`** causes SIGPIPE that silently kills child orchestrators. Redirect to file with `> /tmp/log 2>&1` instead.
- **TensorRT engine recompile** triggered by changing `AXON_EMBED_MICRO_BATCH_MAX_ITEMS` (~5+ min wallclock). Existing cached shape: 128 items.
- **Live runtime default role**: bare `./scripts/axon-live start` picks brain — vector embedding requires explicit `--indexer-full` or `--indexer-vector` (but conflicts with dev GPU work — see DEC-AXO-067 rule 1).
- **Axon MCP tools disconnect** if WSL crashes; remediation: `./scripts/axon-live start [--graph-only]` then use `./scripts/axon-live mcp-call ...` for direct HTTP calls until Claude Code re-connects MCP transport.

## Part 7 — Resume instructions for next session

1. Trigger `axon init` → `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon` first call.
2. Read `kickoff_bundle.active_handoff` (this file).
3. Verify live: `./scripts/axon-live status` — if down, `./scripts/axon-live start --graph-only` (since we are in Axon repo).
4. Resume per priorities:
   - **A** — Implement REQ-AXO-176 P1 (Session entity schema + `axon_session_checkpoint` tool). Highest leverage.
   - **B** — Push REQ-AXO-177 toward 200 chunks/s via path 1 or 2 from Part 4.
   - **C** — Address REQ-AXO-174 (axon-dev rebuild gate) — quick win, ~5 LOC.

## Part 8 — Token economy (this session)

- ~52 K tokens of MCP overhead consumed
- ~9.5% of context used on Axon I/O
- Estimated savings vs no-Axon workflow: 90-170 K tokens (10-15% of 1M window)
- Net benefit: agent stayed lucid at 55% utilisation instead of approaching 75-80%

C'est tout. Bonne session prochaine.
