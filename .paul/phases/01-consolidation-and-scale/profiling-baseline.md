# Profiling Baseline — Plan 01-01

**Date:** 2026-02-27
**Mode:** Full re-index, `--no-embeddings`
**Axon version:** 0.2.3 (pyproject.toml) / post-plan-01-01 batch inserts

## Per-Phase Timings (seconds)

| Phase | machineflow | flow_analyzer | BookingSystem |
|-------|------------|---------------|---------------|
| walk | 1.94 | 1.15 | 0.76 |
| structure | 0.04 | 0.02 | 0.01 |
| parsing | 2.14 | 6.89 | 7.89 |
| imports | 0.01 | 0.01 | 0.01 |
| calls | 0.27 | 0.49 | 0.38 |
| heritage | 0.02 | 0.00 | 0.00 |
| types | 0.01 | 0.01 | 0.02 |
| communities | 0.60 | 0.50 | 0.70 |
| processes | 1.92 | 0.27 | 0.99 |
| dead_code | 0.03 | 0.03 | 0.03 |
| coupling | 0.37 | 0.03 | 0.19 |
| **storage_load** | **477.51** | **589.16** | **535.13** |
| **TOTAL** | **484.9** | **598.7** | **546.2** |

## Repo Metrics

| Repo | Files | Symbols | Relationships | Language |
|------|-------|---------|---------------|----------|
| machineflow | 1,678 | 15,602 | 33,981 | Elixir |
| flow_analyzer | 602 | 12,577 | 41,946 | Elixir |
| BookingSystem | 537 | 12,761 | 30,024 | Elixir |

## Top 3 Bottleneck Phases

1. **storage_load** — 98.5% of total time across all 3 repos
   - machineflow: 477.5s / 484.9s (98.5%)
   - flow_analyzer: 589.2s / 598.7s (98.4%)
   - BookingSystem: 535.1s / 546.2s (98.0%)

2. **parsing** — 0.3-1.4% of total time
   - Largest on BookingSystem (7.89s) and flow_analyzer (6.89s)

3. **walk** — 0.2-0.4% of total time
   - Largest on machineflow (1.94s, 1,678 files)

## Key Insights

- **storage_load is the overwhelming bottleneck.** The `bulk_load` method uses CSV COPY FROM for both nodes and relationships, yet still takes 8-10 minutes per repo. This suggests the bottleneck is inside KuzuDB's COPY FROM processing or the relationship table group structure (N*N table pairs).

- **flow_analyzer has the most relationships (41,946) but fewer files (602)** — high relationship density may explain why it's slower than machineflow despite fewer files.

- **All non-storage phases complete in <15 seconds combined** — the pipeline phases themselves are fast. Optimization effort should focus entirely on the storage layer.

- **Batch inserts (plan 01-01 changes)** improve the incremental path (add_nodes/add_relationships), not the bulk_load path which was already using CSV. The incremental path benefits when re-indexing changed files.

## Comparison: Before vs After Batch Inserts

The batch insert changes (Task 1) affect the **incremental** path only. The full re-index numbers above are the baseline. Future profiling of incremental re-indexing (e.g., changing 1-10 files) will show the improvement from batch inserts vs one-by-one Cypher CREATE.

## Next Steps for Performance

- Investigate KuzuDB bulk_load bottleneck: Is it COPY FROM speed, or the REL TABLE GROUP cartesian product?
- Profile incremental re-index (change 1 file, measure add_nodes + add_relationships time)
- Consider: reducing REL TABLE GROUP pairs (currently N*N node tables)
- Consider: connection pooling or prepared statements for high-frequency operations

---
*Baseline captured: 2026-02-27 during plan 01-01 execution*
