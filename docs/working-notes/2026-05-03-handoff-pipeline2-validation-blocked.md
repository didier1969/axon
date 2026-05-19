# Handoff — 2026-05-03 — Pipeline 2 validation blocked

> Session-private artifact for the next LLM/operator session. Live brain unchanged. SOLL is canonical.

## Part 1 — What this session shipped

| Track | Output | SHA |
|---|---|---|
| Customer-deliverability fixes | REQ-AXO-149/150/151/152/153/155/156 (delivered) | 5bea7ae → dd579c0 (8 commits, pushed) |
| Live build promoted | v0.8.0-160-gdd579c0, healthy, MCP up | — |
| Concept architecture audit | CPT-AXO-026 (pipeline complexity) | SOLL only |
| Decision validation methodology | DEC-AXO-066 (stepwise minimal-first) | SOLL only |
| Bug REQs (NOT FIXED, logged) | REQ-AXO-152, 154, 157-172 (= 16 REQs) | SOLL only |

## Part 2 — Pipeline status (verified)

**Pipeline 1 — Watcher → Graph: ✅ FONCTIONNE**
- Throughput observed: ~10 files/sec, ~10 chunks/sec, ~10 symbols/sec on cold start
- Symbols / Chunks / CALLS / CONTAINS rows growing steadily
- File state machine progresses graph_indexed/indexed correctly

**Pipeline 2 — Graph → Embedding: ❌ TOTALLY BROKEN**
- Zero ChunkEmbedding rows produced after 60s minimum scenario
- `vector_chunks_embedded_total = 0` permanently
- `last_embed_attempt_wall_ms = 0` (the embedder never even *attempted* an embed)
- Embedder subprocess dies on bootstrap → 9-14 zombie processes accumulate
- GPU memory stable at ~700 MiB (only dashboard, BGE-Large model never loaded)
- `semantic_underfeed = true`, `gpu_cadence_underfed` permanent

## Part 3 — Why Pipeline 2 is broken (5 root causes, ranked)

### 1. Embedder subprocess crash silencieux (REQ-AXO-171, P=high)
- 9-14 zombie `[axon-indexer] <defunct>` accumulate during a single session
- No diagnostic log from the dying subprocess
- Operator has no surface to know why embedder failed
- **This is the immediate blocker.**

### 2. AXON_WATCH_DIR env override ignored (REQ-AXO-172, P=high)
- Setting `AXON_WATCH_DIR=/tmp/X` does NOT restrict scan scope
- Indexer scans full /home/dstadel/projects/* despite the env override
- Makes minimal-corpus isolation impossible
- **This blocks DEC-AXO-066 stepwise methodology.**

### 3. Budget gate fires under load (architectural — CPT-AXO-026)
- `exhaustion_ratio > 0.98` pauses vector lane (claim_mode = paused)
- Pipeline 1 initial scan reserves all budget → Pipeline 2 starves
- Self-resolves once Pipeline 1 drains, but takes 10-30 min on full corpus
- Workaround: smaller corpus → blocked by REQ-AXO-172

### 4. Bench orchestration scripts (REQ-AXO-166/167/168, P=high/medium)
- qualify_runtime cold-reset doesn't propagate AXON_RUNTIME_MODE
- dev_baseline_wait grep MCP-only markers indexer cannot produce
- bench script swallows rc=1, exits 0 with empty results.tsv
- **3 bugs the operator stumbles on before reaching the actual pipeline.**

### 5. Other bugs surfaced (already logged)
- REQ-AXO-169: seed-dev-from-live partial copy (2% chunks)
- REQ-AXO-170: main.File row corruption (concat race)
- REQ-AXO-152: brain panic on NULL project_code (FIXED today)
- REQ-AXO-149/150/151/153: customer-deliverability (FIXED today)

## Part 4 — Recommended next-session plan

### Phase 0 — single-bug investigation BEFORE anything else
**Target: REQ-AXO-171 (embedder subprocess crash)**

This is the deepest blocker. Without it, no Pipeline 2 validation possible regardless of corpus size, regardless of budget gate state.

Approach:
1. Don't run the bench script.
2. Don't try to isolate corpus (REQ-AXO-172 blocks that).
3. Start dev `--indexer-full` once, watch the embedder subprocess startup.
4. Use `strace -f -p <main_indexer_pid>` to capture subprocess fork/exec/exit.
5. Use `dmesg` for kernel-level signal logs.
6. Inspect the embedder service code path (`embedder/cuda_service.rs`, `embedder/cpu_query_service.rs`, `embedder/gpu_backend.rs`) to find the bootstrap function and surface its error.

Likely root causes (hypotheses to test):
- ONNX dylib mismatch (env says `/nix/store/4crzkw2iq6abny336ansplbpayq155yw-onnxruntime-1.24.4/lib/libonnxruntime.so` — verify exists)
- BGE-Large model file missing (where does it live? what's the env path?)
- IPC handshake protocol mismatch between brain v0.8.0-160 and embedder
- CPU embedder path not actually wired in indexer_full mode (only in brain via REQ-AXO-128)

### Phase 1 — REQ-AXO-172 (isolation contract)
Once embedder runs at all, fix isolation. Then DEC-AXO-066 stepwise becomes possible.

### Phase 2 — bench orchestration fixes (REQ-AXO-166/167/168)
Lowest priority — do not even attempt before Phase 0+1.

## Part 5 — Critical SOLL entries to read first

- **CPT-AXO-026** : pipeline complexity audit (16 layers, 4 queues)
- **DEC-AXO-066** : stepwise validation methodology (minimal-first protocol)
- **REQ-AXO-171** : embedder subprocess crash (THE blocker)
- **REQ-AXO-172** : AXON_WATCH_DIR ignored (isolation blocker)

## Part 6 — Live state at handoff

- Live brain : v0.8.0-160-gdd579c0 healthy, pid 93340, MCP on port 44129
- Dev : stopped clean, IST wiped (4 KB only)
- Test corpus left at `/tmp/axon-pipeline2-test/sample-project/src/main.rs` (single Rust file, ~50 tokens) — usable as smoke test fixture
- Git: clean tree, 8 commits ahead pushed (cbca185..dd579c0)

## Part 7 — Lessons learned (LLM contract)

1. **Don't debug benchmark before validating pipeline.** This session burned ~3h on bench-script bugs that never mattered because Pipeline 2 was broken at the embedder level.
2. **Don't chase isolation if isolation contract isn't documented.** AXON_WATCH_DIR was assumed to work, didn't.
3. **Read the runtime-heartbeat.json early.** Single source of truth for in-flight queues, scheduler state, embed attempts. Should be the first probe after Ready.
4. **GPU memory is the Pipeline-2 truth telegram.** ~700 MiB = embedder NEVER loaded, regardless of what claim_mode/heartbeat say.
5. **Zombie subprocesses are loud signals.** `ps -ef | grep defunct` early in any pipeline-2 debug.

C'est tout. Bonne session prochaine.
