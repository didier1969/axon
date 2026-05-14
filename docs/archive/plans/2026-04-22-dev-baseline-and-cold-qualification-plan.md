# Dev Baseline and Cold Qualification Plan

## Goal

Create canonical `dev` commands that:
- reset the split runtime into a stable, measurable baseline
- run a cold qualification from that baseline

## Deliverables

1. `scripts/lib/dev-baseline.sh`
   - shared functions for:
     - stopping split runtimes
     - cleaning `IST` dev artifacts
     - cleaning stale dev run roots
     - starting `brain` and `indexer`
     - waiting for split convergence

2. `scripts/reset-dev-baseline.sh`
   - canonical operator entrypoint for:
     - stop
     - clean dev `IST`
     - restart split
     - wait for a stable, measurable baseline:
       - `brain` healthy with fresh feed from `indexer`
       - `indexer` healthy and canonical

3. `scripts/qualify-dev-cold.sh`
   - canonical cold qualification entrypoint
   - calls `reset-dev-baseline.sh`
   - then runs `scripts/qualify_ingestion_run.py` in `--reuse-runtime --mode brain_shadow`

4. `scripts/axon`
   - expose:
     - `reset-dev-baseline`
     - `qualify-dev-cold`

## Validation

1. Shell hygiene:
   - `bash -n scripts/lib/dev-baseline.sh scripts/reset-dev-baseline.sh scripts/qualify-dev-cold.sh scripts/axon`
   - `git diff --check ...`

2. First execution:
   - `bash scripts/reset-dev-baseline.sh`
   - `bash scripts/qualify-dev-cold.sh --duration 20 --interval 5 --label first-cold`

## Definition of Done

1. `reset-dev-baseline.sh` leaves `dev` split converged and healthy.
   - operational interpretation:
     - `indexer` canonical
     - `brain` healthy and connected
     - no unnecessary wait on late `brain` truth promotion
2. `qualify-dev-cold.sh` produces a durable qualification run directory.
3. Commands are available through `scripts/axon`.
