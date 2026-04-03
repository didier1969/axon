# Axon SOTA Ingestion Execution Plan

Date: 2026-04-03
Status: done

## Goal

Make Axon ingestion deterministic, observable per file, and resistant to watcher-driven starvation.

## Phase 1. Canonical File Lifecycle

Status: done

Acceptance:
- `File` carries additive lifecycle truth.
- Every claim/commit/requeue path updates lifecycle fields coherently.
- Structural readiness and vector readiness are queryable independently.

Tasks:
- add `file_stage`, `graph_ready`, `vector_ready`
- backfill lifecycle values on reopen
- update claim, writer-pending, commit, requeue, deleted, oversized transitions
- add regression tests for the new states

## Phase 2. Bounded Subtree Hint Contract

Status: done

Acceptance:
- subtree hints stay deduplicated by path
- blocked subtree events are counted explicitly
- accepted subtree hints are counted explicitly
- runtime can report active vs blocked vs accepted subtree hints

Tasks:
- extend ingress metrics
- expose subtree-hint counters through bridge telemetry
- surface the counters in cockpit and qualification logs

## Phase 3. Structural vs Vector Truth

Status: done

Acceptance:
- graph completion is visible without semantic completion
- vector completion can lag without regressing structural truth
- at least one end-to-end test proves `vector_ready` flips after embeddings land

Tasks:
- keep `graph_ready` on structural commit
- reset `vector_ready` on requeue/reindex/invalidation
- flip `vector_ready` when all current chunks of a file are embedded

## Phase 4. Unified Observability

Status: done

Acceptance:
- cockpit shows `Graph Ready` and `Vector Ready`
- progress backend exposes lifecycle/stage counters
- MCP debug output reports lifecycle counts
- qualification script records lifecycle data

Tasks:
- extend `Progress`
- extend cockpit workspace cards
- extend `axon_debug`
- extend `qualify_ingestion_run.py`

## Phase 5. SOLL Alignment

Status: done

Acceptance:
- SOLL describes the canonical lifecycle target
- bounded subtree-hint control is represented
- structural/vector split is represented
- qualification runs are represented as validation evidence

Tasks:
- update SOLL export snapshot
- restore the snapshot into `soll.db`
- run `axon_validate_soll`

Execution status:
- `bash scripts/stop-v2.sh`
- `bash scripts/start-v2.sh --mcp-only --no-dashboard`
- `devenv shell -- bash -lc 'SQL_URL=http://127.0.0.1:44129/sql bash scripts/apply_sota_ingestion_soll_update.sh'`
- MCP validation call on `/mcp` for `axon_validate_soll` returned:
  - `Validation SOLL: 0 violation(s) de cohérence minimale détectée(s).`
  - `Etat: cohérence minimale vérifiée, 0 violation détectée.`

## Verification

Minimum commands:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
devenv shell -- bash -lc 'cd src/dashboard && mix test'
python3 -m py_compile scripts/qualify_ingestion_run.py
```

Qualification runs:

```bash
python3 scripts/qualify_ingestion_run.py --duration 300 --interval 5 --mode full --label sota-lifecycle
python3 scripts/qualify_ingestion_run.py --duration 300 --interval 5 --mode read_only --no-reset-ist --label sota-read-only
```
