# Brain/Indexer Ops Consolidation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Consolidate the split `axon-brain` / `axon-indexer` operational layer so both runtimes are autonomous, independently startable/stoppable/verifiable, and only coupled through their database contracts.

**Architecture:** Keep the split runtime model already proven on `dev`, but remove residual monolith-era ambiguity from `start/stop/status/release`. Standardize role layout, make local runtime truth primary for split lifecycle probes, and keep release logic explicitly topology-aware instead of relying on legacy fallbacks.

**Tech Stack:** Bash, Python helpers embedded in shell scripts, Rust runtime contracts already present in `src/axon-core`.

---

## Target Invariants

1. `brain` and `indexer` are operationally independent.
2. `brain` owns `SOLL`; `indexer` owns `IST`.
3. `brain` can stay healthy while `indexer` is down.
4. `indexer` can be stopped and restarted without forcing a `brain` restart.
5. `status-brain.sh` and `status-indexer.sh` use local runtime truth first, MCP truth second.
6. Stale sockets, stale pid files, and stale writer lock metadata never report a false healthy split.
7. Release promotion and rollback keep topology explicit: `monolith` vs `split`.
8. Split wrappers (`start-brain.sh`, `start-indexer.sh`, `stop-brain.sh`, `stop-indexer.sh`, `status-brain.sh`, `status-indexer.sh`) are the canonical operator entrypoints for split mode.

## Analysis Findings Driving This Plan

1. `scripts/start.sh` still mixes split startup logic with rollback-to-monolith branches and bin-sync/release concerns.
2. `scripts/status.sh` contains three overlapping truth paths:
   - local indexer fallback
   - local brain fallback
   - MCP-derived topology patch-up
3. `scripts/stop.sh` is mostly split-safe now, but still carries role inference and verification behavior inherited from monolith assumptions.
4. Split role layout is now shared in `scripts/lib/axon-role-layout.sh`, but only partially propagated.
5. `scripts/release/promote_live.sh` and `scripts/release/rollback_live.sh` are topology-aware, yet still center some checks on `bin/axon-core` and a monolithic preflight path.

## Change Set

### Task 1: Consolidate split role layout and entrypoint contracts

**Files:**
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`
- Modify: `scripts/status.sh`
- Review only: `scripts/start-brain.sh`
- Review only: `scripts/start-indexer.sh`
- Review only: `scripts/stop-brain.sh`
- Review only: `scripts/stop-indexer.sh`
- Review only: `scripts/status-brain.sh`
- Review only: `scripts/status-indexer.sh`

**Intent:**
- Eliminate duplicate role-layout logic.
- Make role-specific wrappers the canonical split entrypoints.
- Keep `start.sh` as shared engine, but not as an ambiguous source of truth for split semantics.

**Implementation actions:**
1. Standardize all role-derived paths through `scripts/lib/axon-role-layout.sh`.
2. Remove any duplicated `brain/indexer` path construction from `start/stop/status`.
3. Keep `legacy_monolith` support isolated behind explicit monolith branches only.
4. Preserve wrapper behavior:
   - `brain` => `--read-only`
   - `indexer` => `--full --no-dashboard`

### Task 2: Make split status local-truth-first

**Files:**
- Modify: `scripts/status.sh`

**Intent:**
- Stop inferring split health primarily from stale MCP payloads.
- Use local pid/runtime files/heartbeat/reader replica as the first truth surface for split lifecycle.

**Implementation actions:**
1. For `indexer` role:
   - keep local heartbeat + pid + runtime state as primary truth
   - do not rely on brain-side MCP for readiness
2. For `brain` role:
   - derive `brain_ready` from local pid truth
   - derive `indexer_ready` from local `indexer` pid truth
   - derive `ist_snapshot_state` from local `ist-reader.db` availability when MCP is incomplete
3. Restrict MCP payload usage to:
   - topology authority confirmation
   - version identity confirmation
   - public surface confirmation
4. Collapse contradictory branches so `truth_status`, `canonical_truth_restored`, and `system_converged` are computed once per split truth path.

### Task 3: Make stop verification stale-lock tolerant but still strict

**Files:**
- Modify: `scripts/stop.sh`

**Intent:**
- Treat stale lock metadata as stale metadata, not as a held writer.
- Preserve strict failure on real held writer locks.

**Implementation actions:**
1. Retry `flock` acquisition briefly before declaring failure.
2. If lock metadata references a dead PID and `flock` becomes acquirable, report success.
3. If lock metadata references a dead PID and `flock` still fails after retry, report stale/ambiguous state clearly.
4. Keep split shutdown strict enough that rollback/promotion gates remain blocked on real writer contention.

### Task 4: Isolate monolith compatibility inside explicit branches

**Files:**
- Modify: `scripts/start.sh`
- Modify: `scripts/status.sh`
- Modify: `scripts/stop.sh`

**Intent:**
- Stop letting monolith fallback branches influence normal split operation.

**Implementation actions:**
1. Group monolith logic under explicit `legacy_monolith` branch handling.
2. Keep split default path free of `ROLLBACK_TO_MONOLITH` side effects unless explicitly requested.
3. Ensure split status and stop behavior do not degrade because of monolith-only assumptions.

### Task 5: Align release scripts with split-first operational truth

**Files:**
- Modify: `scripts/release/promote_live.sh`
- Modify: `scripts/release/rollback_live.sh`
- Review only: `scripts/release/check_live_runtime_version.py`

**Intent:**
- Keep live release topology-aware without silently centering monolith artifacts.

**Implementation actions:**
1. Make split promotion/rollback checks explicitly validate:
   - `status-brain.sh`
   - `status-indexer.sh`
   - runtime topology fields
2. Keep monolith artifact preflight only in monolith topology path.
3. Keep split artifact checks centered on `axon-brain` and `axon-indexer`.

## Validation Protocol

Run only after the full consolidation batch is implemented.

### Shell hygiene

Run:
```bash
bash -n scripts/start.sh scripts/stop.sh scripts/status.sh scripts/lib/axon-role-layout.sh \
  scripts/release/promote_live.sh scripts/release/rollback_live.sh
```

Run:
```bash
git diff --check scripts/start.sh scripts/stop.sh scripts/status.sh \
  scripts/lib/axon-role-layout.sh scripts/release/promote_live.sh scripts/release/rollback_live.sh
```

### Runtime validation on `dev`

1. `brain` only:
```bash
env AXON_INSTANCE_KIND=dev bash scripts/stop-indexer.sh
env AXON_INSTANCE_KIND=dev bash scripts/status-brain.sh
```
Expected:
- `brain_ready=true`
- `indexer_ready=false`
- `STATUS HEALTHY`

2. `indexer` only:
```bash
env AXON_INSTANCE_KIND=dev bash scripts/stop-brain.sh
env AXON_INSTANCE_KIND=dev bash scripts/status-indexer.sh
```
Expected:
- `brain_ready=false`
- `indexer_ready=true`
- `STATUS HEALTHY`

3. Full split:
```bash
env AXON_INSTANCE_KIND=dev bash scripts/start-brain.sh
env AXON_INSTANCE_KIND=dev bash scripts/start-indexer.sh
env AXON_INSTANCE_KIND=dev bash scripts/status-brain.sh
env AXON_INSTANCE_KIND=dev bash scripts/status-indexer.sh
```
Expected:
- both `STATUS HEALTHY`
- `system_converged=true`
- `truth_status=canonical`
- `public_mcp_authority=brain`
- `soll_writer_authority=brain`
- `ist_writer_authority=indexer`

4. Restart robustness:
```bash
env AXON_INSTANCE_KIND=dev bash scripts/stop-indexer.sh
env AXON_INSTANCE_KIND=dev bash scripts/start-indexer.sh
env AXON_INSTANCE_KIND=dev bash scripts/status-brain.sh
env AXON_INSTANCE_KIND=dev bash scripts/status-indexer.sh
```
Expected:
- no writer-lock conflict on `IST`
- no stale-lock false failure
- split returns to canonical truth

## Definition of Done

1. Split runtime is autonomous on `dev`.
2. `brain` and `indexer` each have truthful standalone lifecycle probes.
3. `stop-indexer.sh` and `stop-brain.sh` no longer fail on stale local artifacts.
4. `status-brain.sh` and `status-indexer.sh` converge back to canonical truth after restart cycles.
5. Release scripts are explicitly topology-aware and no longer monolith-first in split paths.
