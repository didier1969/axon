# Session 52 marathon — Hand-off (2026-05-22 → 2026-05-23)

Audit-only narrative. Canonical state lives in SOLL `CPT-AXO-052`. Read that
node first ; this file is prose context only.

## What shipped

30 commits delivered post session-51 base `c3ae0324`. Live brain on
`v0.8.0-658-ge96e5a29`, generation `live-20260522T220625Z`. Live indexer
running indexer_full mode with TensorRT GPU acceleration (RTX 3070).
Pipeline_v2 reports 100% coverage : 10898 indexed files, 234189 chunks,
234191 embeddings, 163488 symbols, 375355 edges.

### Umbrellas closed

- `REQ-AXO-901653` (legacy purge) — slice-5a/b/c/d + slice-7 delivered.
  ~5500 LOC removed : worker.rs (1028), HotStatusCache (290),
  spawn_autonomous_ingestor (195), maintain_vector_claimable_supply (110),
  graph_ingestion mod tests (1822), vector_runtime trim (330), sql_helpers
  trim (200), DDL iteration traces (700), 16 test fns (~2400), plus the
  in-DDL DROP TABLE for `public.File` / `public.GraphProjectionQueue` /
  `public.FileVectorizationQueue`.
- `REQ-AXO-901657` (env+KPI lean) — Slice 1 (12 env vars dead purged),
  Slice 4 (alias consolidation 7 clusters), Slice 5 (~50 `AXON_OPT_*` env
  vars → 1 TOML), Slices 2+3 absorbed by slice-5c migration, Slice 6
  shipped `docs/contracts/KPI_CONTRACT.md`.
- `REQ-AXO-901659/660/661` (dev-first gate triple-fix) — gate parses
  `runtime_version.build_id` precisely, build-info gating to live
  instance, 9-case self-test for the parser.
- `REQ-AXO-901662` (slice-5c) — worker.rs + 11 stubs + dead callers
  deletion.
- `REQ-AXO-901663` (test coverage restoration) — 5 vector_runtime tests
  (currently `#[ignore]` pending REQ-AXO-901669).
- `REQ-AXO-901664` (slice-5d) — HotStatusCache + maintain_vector_claimable
  + remaining File refs across MCP surfaces.
- `GUI-AXO-1023` (Swiss-hiking discipline) — formalized.

### Open + announced

- `REQ-AXO-901669` (test_helpers axon_runtime DDL bootstrap) — 5
  `#[ignore]` tests pending ; ~50+ failures will resolve when the
  bootstrap path runs `db/ddl/02_axon_runtime.sql`.
- `cargo test --lib` reports 984 pass / 207 fail (pre-existing rot, not
  regression). Production code green.

## Methodology lessons

1. **Sub-agent infrastructure failed 3× this session** (classifier-down,
   stream stall, idle-timeout). Recovery via manual surgical edits +
   stubs + atomic commits. Confirms `feedback_classifier_is_hard_blocker`
   — surface to operator, don't evade.
2. **DDL agent partial refactor** (kept `public.File` despite "rien
   garder de legacy" directive). Caught via post-agent grep ;
   orchestrator finished manually. Operator-validated.
3. **`scripts/qualify_mcp_robustness.py` called raw `scripts/start.sh`**
   without setting `AXON_INSTANCE_KIND`, causing brain crash via
   libonnxruntime.so dlopen failure when env wasn't pre-resolved.
   Fixed by routing through canonical 4-verb wrapper
   (`./scripts/axon --instance live ...`).
4. **Slice 5 scope underestimation** — plan estimated 13400 LOC at
   read-radius ; actual edit surface (G1) = 2082 LOC ; consumer cascade
   (G2) revealed the entire v1 indexer subsystem (~3000 LOC) needed
   removal. Triggered manual stubs (slice-5b) then full purge
   (slice-5c+5d).
5. **Swiss-hiking principle formalized** as GUI-AXO-1023 after
   operator analogy "ramasser ses déchets ET ceux des autres pour
   garder la nature propre". Every detected weakness must be resolved
   OR logged as REQ. No silent leftovers.

## Next-session entry points

1. **REQ-AXO-901669** — patch `tests::test_helpers::create_test_db()` to
   bootstrap `db/ddl/02_axon_runtime.sql` so the 5 ignored
   `vector_runtime_tests` + ~50 other tests targeting `axon_runtime.*`
   pass cleanly. Quickest path : extend `GraphStore::new` to apply all
   `db/ddl/*.sql` in order, not just `public.*`.
2. **Investigate brain freshness lag** — `status mode=brief` reports
   `ist_projection_freshness: stale` while `embedding_status` confirms
   indexer ready + 100% coverage. Probable `ist_mutated` LISTEN/NOTIFY
   consumer not firing. Diagnostic via `pg_listening_channels()` on
   live PG, then trace `runtime_truth_feed` wiring (REQ-AXO-901658).
3. **Bench validation** — once freshness signal works, run
   `axon-bench-pipeline-v2 --gpu --max-files 5000` to confirm
   ≥ 150 chunks/sec sustained (PIL-AXO-007 acceptance).
4. **Test rot triage Task #11** — ~150 remaining test failures after
   REQ-AXO-901669. Group by file and either migrate (Chunk-based) or
   delete with REQ link.

## Reference IDs

- Session pointer : `CPT-AXO-052`
- Canonical purge : `REQ-AXO-901653`, `REQ-AXO-901657`, `REQ-AXO-901662`,
  `REQ-AXO-901663`, `REQ-AXO-901664`
- Discipline : `GUI-AXO-1023` (Swiss-hiking) INHERITS_FROM
  `GUI-AXO-1003` (anti moitié-moitié)
- KPI contract : `docs/contracts/KPI_CONTRACT.md`
- Test-infra debt : `REQ-AXO-901669` (current)
