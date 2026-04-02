# Ingress Buffer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce a memory-only ingress buffer and batch promoter so watcher/scanner discovery no longer writes every raw event directly into DuckDB.

**Architecture:** Add an `IngressBuffer` in Rust that absorbs raw filesystem discoveries in memory, merges repeated observations by path, then lets an `IngressPromoter` flush reduced batches into canonical `File` updates. Keep `FileIngressGuard`, DuckDB claims, `QueueStore`, and worker execution as the canonical downstream path.

**Tech Stack:** Rust, DuckDB/Canard DB, current Axon watcher/scanner runtime, existing Rust test suite, dashboard read-only telemetry.

---

### Task 1: Freeze the architecture contract in docs and tests

**Files:**
- Modify: `task_plan.md`
- Modify: `findings.md`
- Modify: `progress.md`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the failing tests**

Add tests that describe the new contract without integrating the runtime path yet:

- multiple watcher events for the same file collapse to one promotable ingress event
- highest observed priority wins inside the in-memory buffer
- file delete beats stale old metadata
- directory hint does not recursively stage a subtree directly
- promoter flush writes only reduced decisions into DuckDB
- crash model does not require durable ingress replay

**Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml ingress_buffer -- --nocapture'
```

Expected:

- compile error or failing tests because the ingress buffer module does not exist yet

**Step 3: Update planning artefacts**

Record:

- why discovery and canonical `pending` are now separated
- why MVP remains memory-only
- why `QueueStore` is not replaced

**Step 4: Commit**

```bash
git add task_plan.md findings.md progress.md src/axon-core/src/tests/maillon_tests.rs
git commit -m "test: freeze ingress buffer contract"
```

### Task 2: Add the isolated memory-only buffer module

**Files:**
- Create: `src/axon-core/src/ingress_buffer.rs`
- Modify: `src/axon-core/src/lib.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write minimal implementation**

Add:

- `IngressBuffer`
- `IngressEvent`
- `IngressCause`
- `IngressSource`
- `IngressDecision` or equivalent promotion-facing shape
- merge/collapse logic by path
- subtree hint storage without recursive restaging
- explicit `kill switch`, for example `AXON_ENABLE_INGRESS_BUFFER`

Keep it in-memory only.

**Step 2: Run focused tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml ingress_buffer -- --nocapture'
```

Expected:

- buffer contract tests pass
- no runtime integration yet

**Step 3: Commit**

```bash
git add src/axon-core/src/ingress_buffer.rs src/axon-core/src/lib.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add isolated ingress buffer"
```

### Task 3: Introduce the batch promoter and canonical batch API

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/ingress_buffer.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the failing tests**

Cover:

- batch promotion writes only one canonical `pending` update for repeated file events
- tombstone promotion marks deleted canonically
- highest observed priority is retained
- `status_reason` after promotion stays explicit
- unchanged file filtered by `FileIngressGuard` does not get promoted

**Step 2: Run targeted tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml ingress_promoter -- --nocapture'
```

Expected:

- failure before new batch promotion API exists

**Step 3: Implement canonical batch promotion**

Add a single batch-oriented path in `GraphStore`, for example:

- `promote_ingress_batch(...)`

It should:

- take reduced ingress decisions
- write canonical `File` updates in one transaction or chunked transactions
- re-read committed `File` rows when needed so `FileIngressGuard` can still learn from committed truth only

**Step 4: Re-run tests**

Run the same targeted suite and confirm it passes.

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/graph.rs src/axon-core/src/ingress_buffer.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add canonical ingress batch promotion"
```

### Task 4: Add the promoter loop to the runtime without changing scheduler authority

**Files:**
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/bridge.rs`
- Modify: `src/axon-core/src/main_telemetry.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the failing tests**

Cover:

- promoter loop flushes hot events on a short timer
- promoter loop flushes scan events on count or timer thresholds
- promoter loop updates guard from committed rows only
- DuckDB claim path remains unchanged after promotion

**Step 2: Run focused tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml promoter_loop -- --nocapture'
```

Expected:

- failing runtime integration tests before the loop is wired

**Step 3: Wire the runtime**

Add:

- shared `IngressBuffer`
- shared `IngressPromoter`
- flush policy:
  - hot flush window
  - bulk flush window
  - batch-size flush

Do not change:

- claim ordering
- `QueueStore`
- worker pool

**Step 4: Extend telemetry**

Expose at least:

- ingress buffered entries
- flush count
- dropped/collapsed event count
- last flush duration
- last promoted count

**Step 5: Re-run focused tests**

Run the same suite and confirm it passes.

**Step 6: Commit**

```bash
git add src/axon-core/src/main.rs src/axon-core/src/main_background.rs src/axon-core/src/bridge.rs src/axon-core/src/main_telemetry.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add ingress promoter runtime loop"
```

### Task 5: Convert the watcher into a pure ingress producer

**Files:**
- Modify: `src/axon-core/src/fs_watcher.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the failing tests**

Cover:

- file hot delta becomes buffer event, not direct DB write
- repeated events on the same file collapse in memory
- directory events become subtree hints, not recursive hot staging
- missing files become tombstone ingress events
- watcher stays fail-open if buffer is disabled

**Step 2: Run targeted watcher tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml watcher_ingress_buffer -- --nocapture'
```

Expected:

- watcher integration tests fail before conversion

**Step 3: Wire watcher producer behavior**

Replace direct canonical writes with:

- enqueue file ingress event
- enqueue tombstone ingress event
- enqueue subtree hint for directory events

Do not recursively restage a directory directly in the watcher hot path.

**Step 4: Re-run targeted watcher tests**

Confirm the watcher now behaves as a producer only.

**Step 5: Commit**

```bash
git add src/axon-core/src/fs_watcher.rs src/axon-core/src/main_background.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: convert watcher to ingress producer"
```

### Task 6: Convert the scanner into a bulk ingress producer

**Files:**
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`
- Test: `src/axon-core/src/tests/pipeline_test.rs`

**Step 1: Write the failing tests**

Cover:

- initial scan populates ingress buffer instead of writing every file immediately
- repeated scan observations collapse before DB promotion
- unchanged known files are filtered cheaply through `FileIngressGuard`
- changed files still become promotable decisions

**Step 2: Run targeted scanner tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml scanner_ingress_buffer -- --nocapture'
```

Expected:

- scanner integration tests fail before conversion

**Step 3: Wire scanner producer behavior**

Change scanner batching so it:

- builds ingress events
- lets the promoter decide when to write canonical `pending`
- still honors `.axonignore`

**Step 4: Re-run targeted scanner tests**

Confirm scan behavior is preserved while reducing canonical write frequency.

**Step 5: Commit**

```bash
git add src/axon-core/src/scanner.rs src/axon-core/src/main_background.rs src/axon-core/src/tests/maillon_tests.rs src/axon-core/src/tests/pipeline_test.rs
git commit -m "feat: convert scanner to ingress producer"
```

### Task 7: Make project completeness and backlog truth survive the new ingress layer

**Files:**
- Modify: `src/axon-core/src/mcp/tools_system.rs`
- Modify: `src/axon-core/src/mcp/tools_dx.rs`
- Modify: `src/axon-core/src/mcp/tools_risk.rs`
- Modify: `src/axon-core/src/mcp/tools_governance.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing tests**

Cover:

- `axon_debug` distinguishes:
  - ingress buffered
  - canonical pending
  - indexing
- scope completeness remains honest while promotion is in flight
- operator truth is not overstated during large ingress bursts

**Step 2: Run targeted MCP tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml mcp::tests -- --nocapture'
```

Expected:

- failing assertions before MCP reflects buffered-vs-canonical truth

**Step 3: Implement read-side truth**

Expose:

- buffered ingress count
- canonical pending count
- indexing count
- completed count
- note when backlog is still mostly in ingress memory rather than canonical `pending`

**Step 4: Re-run MCP tests**

Confirm the operator/MCP truth remains honest.

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_system.rs src/axon-core/src/mcp/tools_dx.rs src/axon-core/src/mcp/tools_risk.rs src/axon-core/src/mcp/tools_governance.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: expose ingress buffer truth in MCP diagnostics"
```

### Task 8: Run the full validation gate and compare before/after behavior

**Files:**
- Modify: `progress.md`
- Modify: `findings.md`
- Modify: `task_plan.md`
- Modify: `docs/working-notes/2026-04-01-wsl-install-runtime-notes.md`

**Step 1: Run full verification**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'
devenv shell -- bash -lc 'cd src/dashboard && mix test'
bash scripts/stop-v2.sh
bash scripts/start-v2.sh
devenv shell -- bash -lc 'python3 scripts/monitor_runtime_v2.py --duration 60 --interval 1 --csv /tmp/axon_ingress_buffer_validation.csv'
bash scripts/stop-v2.sh
```

Expected:

- full tests pass
- start/stop stays green
- monitor shows fewer direct canonical write bursts
- watcher no longer recursively restages whole directory subtrees as hot deltas
- canonical `pending` becomes more meaningful

**Step 2: Write the comparison**

Record:

- ingress buffer counts
- canonical pending/indexing evolution
- whether MCP availability improves
- whether startup churn decreases

**Step 3: Commit**

```bash
git add progress.md findings.md task_plan.md docs/working-notes/2026-04-01-wsl-install-runtime-notes.md
git commit -m "docs: record ingress buffer rollout validation"
```
