# SOLL Closure — Concept Document

**Date:** 2026-04-30
**Scope:** Close 15 partial + 2 missing SOLL requirements for AXO
**Triage:** `full` (cross-cutting: GPU runtime, process supervision, benchmarks, SOLL hygiene)

## Current State

SOLL verification: **42 done, 15 partial, 2 missing** (out of 59 requirements).

## Three Delivery Tracks

### Track A: SOLL Metadata Cleanup (no code changes)

| Action | Requirements | Rationale |
|--------|-------------|-----------|
| Deduplicate | REQ-AXO-058 (dup of REQ-AXO-050), REQ-AXO-059 (dup of REQ-AXO-056) | Exact duplicates in description and scope |
| Status update | REQ-AXO-050 → `accepted`, REQ-AXO-053 → verify .gitignore then close | Code already delivered |
| Evidence attach | VAL-AXO-010 (topology regression), VAL-AXO-016 (telemetry partial) | Existing proofs can close these |
| Acceptance criteria | REQ-AXO-054, REQ-AXO-055 (already have them from latest SOLL export) | Criteria exist, verify completeness |

### Track B: Code Changes (actionable now)

#### B1: REQ-AXO-049 — VRAM Guard (ENABLED by default)

**Finding:** All 4 acceptance criteria are implemented in code:
1. VRAM sampled via NVML/nvidia-smi before each GPU batch (`gpu_telemetry.rs`)
2. Batch requeued when used VRAM exceeds admission (`gpu_policy.rs::gpu_primary_batch_allowed`)
3. Worker recycled on VRAM plateau (`gpu_pre_batch_vram_recycle_reason()` in embedder.rs:2632-2686)
4. Backpressure as controlled requeue (`gpu_worker_consumption_allowed()`)

**Problem:** `AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED` defaults to `false`.

**Action:**
- Enable pre-batch VRAM guard by default in GPU-capable runtimes
- Add unit test for `gpu_pre_batch_vram_recycle_reason()` decision logic
- Attach evidence and close requirement

#### B2: REQ-AXO-051 — SIGTERM Parent-First (test)

**Finding:** Parent-first SIGTERM is fully implemented:
- axonctl.rs: SIGTERM roots → 200ms grace → SIGTERM children → 1500ms timeout → SIGKILL
- stop.sh: Same strategy as shell fallback
- Zero tests exist

**Action:**
- Write integration test spawning real processes with signal handlers
- Verify parent-first ordering prevents child respawn
- Attach evidence and close requirement

#### B3: REQ-AXO-052 — axonctl Typed Supervision

**Finding:** axonctl stop-tree works, scripts delegate with fallback.

**Action:** Verify current state satisfies acceptance criteria, attach evidence, close.

#### B4: REQ-AXO-055 — axonctl in Release Artifacts

**Action:**
- Add axonctl to Cargo build targets alongside axon-brain/axon-indexer
- Include in release manifest creation (scripts/release/create_manifest.py)
- Add preflight check for axonctl presence

### Track C: Hardware-Dependent (blocked without GPU runtime)

| Requirement | Blocker |
|-------------|---------|
| REQ-AXO-057 (30 chunks/s) | Needs GPU runtime + TensorRT qualification |
| VAL-AXO-004-009 | Various runtime qualification probes |
| VAL-AXO-026 | TensorRT VRAM bounded qualification |

**Action:** Document as `blocked_by: gpu_runtime` with clear unblock criteria. Do not pretend these are closeable without hardware evidence.

## Non-Goals

- Do not change GPU batch sizing or token thresholds
- Do not modify TensorRT build infrastructure
- Do not refactor embedder.rs monolith (REQ-AXO-034 is separate)
- Do not modify MCP tool surface (MIL-AXO-009 is separate)

## Risk Assessment

| Risk | Mitigation |
|------|-----------|
| Enabling VRAM guard by default reduces throughput | Guard only triggers above admission threshold; no change below |
| Integration test for SIGTERM flaky on CI | Use generous timeouts, test on local only initially |
| Deduplicating requirements breaks SOLL links | Use soll_manager update to mark superseded, preserve links |

## Success Criteria

- SOLL verification improves from 42/59 done to ≥50/59 done
- No code regressions (cargo test passes)
- Hardware-dependent items explicitly documented as blocked
