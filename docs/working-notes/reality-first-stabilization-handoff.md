---
title: Reality-First Stabilization Handoff
date: 2026-03-30
branch: feat/rust-first-control-plane
status: in-progress
---

# Scope

This handoff exists to preserve exact working state across context compaction.
Do not rely on conversational memory before resuming work. Re-read this file, then verify the repository state with Git and tests.

# Current Objective

Stabilize Axon for real daily use before further sophistication:

1. Make `Nix + Devenv` the operational source of truth.
2. Harden Rust native ingestion and MCP/SOLL paths.
3. Turn visible operator and MCP surfaces into truthful, useful workflows for LLM-assisted development.
4. Keep progress measurable with concrete validation, not impression.
5. Only then continue with dashboard quality and broader architecture cleanup.

# Branch

Current working branch:

`feat/rust-first-control-plane`

This branch was created after merging the previous stabilization wave into `main`.

Historical base branch used earlier:

`feature/axon-native-ingestion`

# Skills Explicitly Used As Methodology

- `/home/dstadel/.claude/skills/mission-critical-architect/SKILL.md`
- `/home/dstadel/.claude/skills/system-observability-tracer/SKILL.md`
- `/home/dstadel/.claude/skills/hardware-aware-scaling/SKILL.md`
- `/home/dstadel/.claude/skills/devenv-nix-best-practices/SKILL.md`
- `/home/dstadel/projects/axon/.claude/skills/axon-digital-thread/SKILL.md`
- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`

Skills created during this work:

- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`
- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/agents/openai.yaml`
- `/home/dstadel/projects/axon/.claude/skills/axon-digital-thread/agents/openai.yaml`

# Method Being Applied

The workflow used so far is:

1. Understand the project vision and architecture before editing.
2. Separate vision, intended architecture, actual code, and actual runtime behavior.
3. Validate the real development environment before trusting diagnostics.
4. Prioritize dominant stability defects over exhaustive low-value cleanup.
5. Fix foundations first: environment, storage bootstrap, atomic claiming, protocol correctness, test reliability.
6. Measure progress after each phase with concrete test signals.
7. Prefer validation en conditions reelles over speculative product promises.

# Current Architecture Reference

For the next ingestion redesign, the active concept document is:

- `/home/dstadel/projects/axon/docs/architecture/2026-03-30-adaptive-ingestion-concept.md`
- `/home/dstadel/projects/axon/docs/architecture/2026-03-30-rust-first-elixir-visualization.md`

# Current Architecture Decision

Axon now targets this split:

- Rust = canonical runtime plane
- Elixir/Phoenix = visualization and operator plane

`SOLL` also gained a first executable read-only coherence layer via `axon_validate_soll`:

- `Requirement` must not be fully orphaned
- `Validation` must verify something
- `Decision` must link to a need or impact
- validation is advisory only and stays compatible with historical merge restore
- the official Markdown `export -> restore` path now replays metadata and explicit SOLL links when they are present in the export, while keeping historical exports valid through optional append-only sections
- the next thin governance prototype is now selected: keep `soll.db` canonical, keep timestamped snapshots, and add only a derived per-item `current` disk view for stable-ID entities (`Requirement`, `Decision`, `Validation`)

This means the ongoing migration must remove remaining ingestion/control-plane authority from Elixir while preserving the UI.

## Residual Elixir Authority Frozen For Wave 1

The inventory is now explicit and must be treated as migration debt, not product architecture.

Marked `to retire` as canonical ingestion/control authority:

- `Axon.Watcher.Server`
- `Axon.Watcher.Staging`
- `Axon.Watcher.PathPolicy`
- `Axon.Watcher.IndexingWorker`
- `Axon.Watcher.BatchDispatch`
- `Axon.Watcher.PoolFacade`
- `Axon.Watcher.PoolEventHandler`
- `Axon.BackpressureController`
- `Axon.Watcher.TrafficGuardian`

Allowed to remain as visualization/operator consumers unless later evidence says otherwise:

- `Axon.Watcher.CockpitLive`
- `Axon.Watcher.Progress`
- `Axon.Watcher.Telemetry`
- `Axon.Watcher.StatsCache`
- `Axon.Watcher.Auditor`
- `Axon.Watcher.SqlGateway`

Wave-1 removal order is now fixed:

1. remove canonical pressure authority from Elixir
2. remove canonical watcher gating and staging/dispatch authority from Elixir
3. collapse `batch_dispatch -> pool_event_handler -> indexing_worker -> staging`
4. reduce bridge modules to read/telemetry-only duties
5. preserve UI behavior while Rust remains sole runtime truth

Wave-1 constraint now discovered:

- `BackpressureController` can be display-only only as a transitional state
- because `server -> staging -> Oban -> indexing_worker -> parse_batch` still exists today
- therefore Task 3 cannot be delayed far behind Task 2
- explicit operator `trigger_scan` may still be relayed to Rust
- but Elixir must no longer force a local `indexing` overlay before Rust/DB truth confirms it
- Elixir may display pressure, but Rust must remain the only canonical throttling authority

# High-Value Findings Identified Earlier

These were the dominant issues initially identified:

1. DuckDB plugin resolution depended on `cwd` and broke tests/runtime.
2. `pending -> claimed -> indexed` flow was not atomic.
3. Batch ACK semantics were not safely correlated.
4. Some Elixir audit/bridge paths were stale or inconsistent with current runtime.
5. MCP audit/health outputs overstated confidence while relying on stubs.
6. Ingestion still contained artificial throttling and blocking patterns.

This list was intentionally prioritized, not exhaustive.

# Changes Already Made

## Environment / Devenv

Files changed:

- `/home/dstadel/projects/axon/flake.nix`
- `/home/dstadel/projects/axon/devenv.yaml`
- `/home/dstadel/projects/axon/devenv.nix`
- `/home/dstadel/projects/axon/flake.lock`
- `/home/dstadel/projects/axon/devenv.lock`
- `/home/dstadel/projects/axon/README.md`
- `/home/dstadel/projects/axon/scripts/setup_v2.sh`
- `/home/dstadel/projects/axon/scripts/start-v2.sh`
- `/home/dstadel/projects/axon/scripts/stop-v2.sh`
- `/home/dstadel/projects/axon/scripts/validate-devenv.sh`

What changed:

- Shifted setup and start scripts to `devenv shell`.
- Re-aligned stop script with the current local runtime instead of the old DB bootstrap path.
- Added explicit environment validation script.
- Updated README to point contributors at `devenv shell` as the primary path.
- HydraDB was intentionally detached from the current Axon Devenv workflow.
- Active HydraDB coupling was removed from `flake.nix`, `devenv.nix`, `devenv.yaml`, `flake.lock`, and `devenv.lock`.
- `axon-db-start` is now a guarded placeholder instead of a live dependency path.
- `setup_v2.sh` and `start_v2.sh` were corrected to use the Devenv `CARGO_TARGET_DIR` output path instead of the stale `src/axon-core/target/...` assumption.
- `start_v2.sh` now also fails fast if it detects a newer `src/axon-core/target/release/axon-core` built outside Devenv, because that path does not feed the runtime launched by the official start script.
- The stable operational rule is now explicit: if Axon is started through `start_v2.sh`, the authoritative release binary must come from `.axon/cargo-target/release/axon-core`, built inside `devenv shell`.
- `start_v2.sh` now attempts the Devenv rebuild automatically before falling back to a manual instruction.
- `start_v2.sh` now performs a live SQL schema probe after the core port opens, so a false-positive runtime start is rejected if `/sql` does not expose the expected tables.
- `Axon Ignore` redesign is now started with a real hierarchical `.axonignore` / `.axonignore.local` path in the Rust core scanner, validated by test, and aligned with the dashboard NIF scanner without hardcoded absolute ignore paths.
- the Rust autonomous ingestor no longer uses a fixed `queue.len() < 5000` / `fetch_pending_batch(2000)` policy only; it now applies an adaptive claim policy based on queue pressure, memory pressure, and recent live `/sql` + `/mcp` latency
- a new `service_guard` module records recent MCP and SQL latency so bulk claiming can slow down before live service becomes unresponsive
- the Rust claim policy now exposes explicit canonical modes:
  - `fast`
  - `slow`
  - `guarded`
  - `paused`
  and logs transitions so the operator plane can consume runtime truth instead of inventing a second throttling authority
- the `IST` restart policy is no longer binary:
  - additive repair preserves compatible `File` state
  - ingestion drift now soft-invalidates derived structural layers and requeues `File`
  - embedding drift now soft-invalidates semantic layers only
  - hard rebuild is reserved for incompatible base `File` schema
- the semantic worker is no longer unconditional work: embeddings now pause under queue pressure or recent live service degradation, making semantic enrichment a true slack-driven layer instead of a permanent competitor to structural ingestion
- the scanner itself now applies adaptive discovery throttling based on pending backlog, memory pressure, and recent live `/sql` + `/mcp` latency; discovery is no longer a blind fixed-rate producer
- the Rust core now computes a host-specific runtime profile at boot and uses it to size worker count, queue capacity, blocking-thread budget, and RAM headroom instead of relying on fixed constants
- the dashboard resource monitor now computes real `io` pressure from `/proc/stat` deltas instead of always publishing `0.0`
- the first Rust-native delta staging slice now exists through `fs_watcher::stage_hot_delta(...)`, which re-stages changed files directly into `IST`, enforces hierarchical `Axon Ignore`, and promotes the delta with hot priority without relying on Elixir watcher authority
- this slice is now wired to a native Rust filesystem watcher in `main_background.rs`, using debounced OS events to feed the same durable `IST` backlog path
- duplicate bursts and missing short-lived paths are now tolerated by the Rust watcher path instead of surfacing as runtime failures
- the watcher no longer assumes that the whole universe can be armed as one clean recursive watch target; it now skips unreadable immediate project roots instead of letting a single bad subtree poison the global arming step
- the active project root is now prioritized when computing watcher targets, so the hot set can be armed before the full universe finishes its recursive registration
- the hot watcher path is now explicitly split into:
  - universe root non-recursive watcher
  - active project root non-recursive watcher
  - active project visible child subtrees as recursive hot targets
- the Rust boot sequence now pre-indexes `AXON_PROJECT_ROOT` before launching the full universe scan, so the current repo becomes visible in `IST` earlier
- the watcher bootstrap suppression window now also covers the short period immediately after the cold universe finishes arming, so a delayed registration storm does not trigger a destructive safety rescan right after the watcher becomes fully armed
- hot deltas no longer reopen a file already `indexing` just to raise its priority; identical hot re-observation keeps the active claim in place
- real metadata drift observed during `indexing` is now preserved through a `needs_reindex` flag, so the file is replayed once after the current commit instead of being claimed twice concurrently
- non-qualified top-level symbols are now path-aware in `Symbol.id`, which avoids cross-file collisions for helpers such as repeated `send_cypher` functions inside the same project
- legacy `IST` files are now repaired additively at boot for `needs_reindex` before runtime-compatibility logic runs, so a narrow schema drift no longer causes `Binder Error` loops during live restart
- the Rust watcher now emits explicit checkpoints on the hot-delta path (`watcher.storm_suppressed`, `watcher.storm_salvaged`, `watcher.received`, `watcher.filtered`, `watcher.db_upsert`, `watcher.staged`) through a shared in-memory probe buffer plus runtime logs
- the Rust watcher now also emits explicit rescan and failure checkpoints:
  - `watcher.rescan_requested`
  - `watcher.rescan_started`
  - `watcher.rescan_completed`
  - `watcher.rescan_skipped`
  - `watcher.tombstoned`
  - `watcher.staged_none`
  - `watcher.staging_failed`
  - `watcher.error`
- bootstrap-storm salvage is now restricted to active-project file paths only; whole directories from a startup storm are no longer recursively restaged inside the watcher callback
- a missing watcher path no longer dies as a blind no-op when it maps to known `IST` truth:
  - delete and rename now tombstone the old `File` path
  - derived truth for that path is purged immediately
  - a late worker commit can no longer resurrect a tombstoned path
  - the new rename target is staged as an ordinary hot delta
- startup now salvages interrupted claims too:
  - `File.status='indexing'` rows are moved back to `pending`
  - `worker_id` is cleared
  - replay can resume without requiring a full rescan or an `IST` version drift
- the RAM scheduler now carries explicit `hot / bulk / titan` lanes:
  - `hot` keeps reserved capacity and drains first
  - `bulk` hits backpressure before `hot`
  - `titan` is isolated from the common lane so oversized files do not poison ordinary throughput
  - canonical claim pressure now follows `hot + bulk`, not the isolated `titan` backlog
  - if a lane is saturated, the claimed file is requeued to `pending` in `IST` instead of being dropped
  - the remaining limit is now explicit:
    - priority is proven at queue drain level
    - a stricter end-to-end fairness bound at worker prefetch level remains for a later slice if needed
- live service health is now modeled explicitly inside Rust:
  - `Healthy`
  - `Recovering`
  - `Degraded`
  - `Critical`
- this state is now canonical for both structural claiming and semantic work:
  - claim depth no longer reacts only to a raw latency number
  - recovery back to full throughput is gradual instead of on/off
  - embeddings pause before structural ingestion is fully stopped
  - the semantic worker now follows the same common-lane pressure signal as the structural ingestor
  - low-latency samples no longer clear pressure instantly; a bounded cooldown keeps the runtime in `Recovering` before it returns to `Healthy`

## Rust Core / Native Ingestion / MCP

Files changed:

- `/home/dstadel/projects/axon/src/axon-core/src/graph.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main_background.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main_services.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main_telemetry.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/fs_watcher.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/graph_analytics.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/graph_query.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/parser/go.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/parser/mod.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/parser/sql.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/scanner.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/worker.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/tests/bench_extraction.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/tests/maillon_tests.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/tests/pipeline_test.rs`

What changed:

- DuckDB plugin path resolution now uses robust repo-relative discovery instead of fragile `cwd` assumptions.
- `GraphStore` bootstrap was hardened for `ist.db` and attached `soll.db`.
- live SQL/MCP reads now use the writer connection path instead of a stale separate reader snapshot, because the previous split could return `File=0` and `Chunk=0` while ingestion was actively writing.
- this writer-backed live read path is now validated both by an on-disk red/green test and by the real runtime launched through `start-v2.sh`.
- In-memory DB handling was adjusted to avoid read-only attach failure patterns.
- `fetch_pending_batch()` was changed to claim work atomically under transaction.
- `worker_id` is now cleared when files transition to terminal states.
- SQL parameter handling now supports positional `?` arguments in addition to named params.
- the Rust core now contains a first watcher-facing delta staging API:
  - `fs_watcher::stage_hot_delta(...)`
  - `fs_watcher::stage_hot_deltas(...)`
  - `GraphStore::upsert_hot_file(...)`
  - this path reuses the same durable `IST` backlog instead of routing changed files through Elixir-first authority
- `main_background.rs` now starts a native debounced filesystem watcher and routes events into `fs_watcher::stage_hot_deltas(...)`
- watcher-side safety rescan support is now present for `need_rescan()` events, guarded to avoid concurrent safety rescans
- watcher target selection now:
  - keeps the universe root in non-recursive mode
  - arms readable immediate project roots recursively
  - skips unreadable roots
  - prioritizes `AXON_PROJECT_ROOT` before other projects
- hierarchical `Axon Ignore` is now enforced both for full discovery and for Rust-side hot delta staging
- MCP/SOLL test expectations were updated to match the current schema and export behavior.
- Several previously stub-like audit/health helpers in `graph.rs` were replaced with real graph-derived signals.
- `axon_query` now reports its effective mode instead of overstating semantic availability.
- `axon_restore_soll` is being integrated as the official MCP-driven restore path from `SOLL_EXPORT_*.md`.
- One sandbox-sensitive Unix socket test was made robust by skipping only on `PermissionDenied`.
- Rust warnings were reduced by removing or renaming unused code and imports.

# Validation Signals Achieved

Rust validation reached a clean state during this session:

- `cargo test` in `/home/dstadel/projects/axon/src/axon-core`
- result reached first: `26 passed; 0 failed`
- result reached now: `27 passed; 0 failed`
- result reached now after VCR coverage expansion: `30 passed; 0 failed`
- result reached now after reader/writer consistency coverage: `36 passed; 0 failed`
- result reached now after Axon Ignore unification and adaptive claim policy tests: `38 passed; 0 failed` in `src/lib.rs` and `6 passed; 0 failed` in `src/main.rs`
- result reached now after service latency guard integration: `40 passed; 0 failed` in `src/lib.rs` and `8 passed; 0 failed` in `src/main.rs`
- result reached now after slack-driven semantic policy: `43 passed; 0 failed` in `src/lib.rs` and `8 passed; 0 failed` in `src/main.rs`
- result reached now after adaptive discovery throttling: `47 passed; 0 failed` in `src/lib.rs` and `8 passed; 0 failed` in `src/main.rs`
- result reached now after runtime profile integration: `50 passed; 0 failed` in `src/lib.rs` and `8 passed; 0 failed` in `src/main.rs`
- result reached now after first Rust-native delta watcher staging slice: `53 passed; 0 failed` in `src/lib.rs` and `8 passed; 0 failed` in `src/main.rs`
- result reached now after native debounced watcher wiring and hot-delta hardening: `55 passed; 0 failed` in `src/lib.rs` and `9 passed; 0 failed` in `src/main.rs`
- result reached now after watcher target prioritization and unreadable-root tolerance: `55 passed; 0 failed` in `src/lib.rs` and `12 passed; 0 failed` in `src/main.rs`
- result reached now after hot-target split, bootstrap-storm suppression, and active-project pre-scan: `57 passed; 0 failed` in `src/lib.rs` and `17 passed; 0 failed` in `src/main.rs`
- result reached now after delayed cold-arm storm suppression: `57 passed; 0 failed` in `src/lib.rs` and `19 passed; 0 failed` in `src/main.rs`
- result reached now after active-claim preservation and path-aware top-level symbol IDs: `61 passed; 0 failed` in `src/lib.rs` and `19 passed; 0 failed` in `src/main.rs`
- result reached now after additive legacy-`IST` schema repair for `needs_reindex`: `62 passed; 0 failed` in `src/lib.rs` and `19 passed; 0 failed` in `src/main.rs`
- result reached now after watcher probes and file-only bootstrap salvage: `63 passed; 0 failed` in `src/lib.rs` and `21 passed; 0 failed` in `src/main.rs`
- result reached now after explicit `IST` invalidation policy tests: `66 passed; 0 failed` in `src/lib.rs` and `25 passed; 0 failed` in `src/main.rs`
- result reached now after watcher rescan/no-op/error checkpoint coverage: `66 passed; 0 failed` in `src/lib.rs` and `30 passed; 0 failed` in `src/main.rs`
- result reached now after delete/rename tombstone handling and crash-mid-index replay: `70 passed; 0 failed` in `src/lib.rs` and `30 passed; 0 failed` in `src/main.rs`
- result reached now after explicit `hot / bulk / titan` queue lanes: `73 passed; 0 failed` in `src/lib.rs` and `31 passed; 0 failed` in `src/main.rs`
- result reached now after live-service health states and gradual recovery policy: `79 passed; 0 failed` in `src/lib.rs` and `32 passed; 0 failed` in `src/main.rs`
- dashboard validation remains green after real `io` monitoring work: `31 tests, 0 failures`

Runtime note:

- live boot now shows `Rust FS watcher preparing targets under /home/dstadel/projects`
- live boot now also shows:
  - immediate arming of `/home/dstadel/projects`
  - immediate arming of `/home/dstadel/projects/axon`
  - recursive arming of active-project hot subtrees such as `/src`, `/tests`, `/docs`, `/scripts`
  - suppression of early bootstrap storms instead of immediate safety rescan
  - suppression of the delayed cold-arm event burst instead of an immediate post-arm safety rescan
- live boot now pre-indexes the active repo before the full universe scan:
  - `Hot subtree scan complete: 366 files mapped from "/home/dstadel/projects/axon"`
  - `SELECT count(*) FROM File WHERE path LIKE '/home/dstadel/projects/axon/%'` returned `366` while the universe scan was still in progress
- the previous live proof gap is now closed:
  - explicit watcher-driven delta insertion for newly created files in the active repo has now been observed end-to-end in runtime logs and confirmed via `/sql`
- one new live defect is now isolated for the next slice:
  - the previous `Duplicate key` failures on top-level helper symbols were traced to cross-file `Symbol.id` collisions and to hot deltas reopening active claims
  - both are now covered by executable Rust tests and corrected in the commit path
- the previous live restart defect on legacy `IST` is now isolated and corrected too:
  - older `File` tables without `needs_reindex` no longer enter `Binder Error` loops
  - additive boot migration repairs the column before claim/reopen paths execute
- the explicit watcher live proof is now available:
  - runtime logs show `watcher.db_upsert` then `watcher.staged` on real files in the active repo
  - `/sql` confirms those same rows in `File` with `status='indexed'` and `priority=900`
  - verified examples:
    - `/home/dstadel/projects/axon/src/watcher_src_probe.ex`
    - `/home/dstadel/projects/axon/tmp/rust_watcher_live.ex`
    - `/home/dstadel/projects/axon/tmp/rust_watcher_live_two.ex`
    - `/home/dstadel/projects/axon/tmp/rust_watcher_live_three.ex`
    - `/home/dstadel/projects/axon/tmp/rust_watcher_live_final.ex`
    - `/home/dstadel/projects/axon/tmp/rust_watcher_live_success.ex`
- watcher observability is now broader in live logs too:
  - `watcher.storm_suppressed`
  - `watcher.storm_salvaged`
  - `watcher.received`
  - `watcher.filtered`
  - `watcher.db_upsert`
  - `watcher.staged`
  were observed again from the real runtime pane after restart

Important note:

- This signal was obtained after stabilizing DuckDB path resolution, SOLL schema gaps, MCP behavior, and the sandbox-sensitive socket test.
- A new persistent on-disk test now proves `writer_ctx -> query_count/query_json -> reopen` consistency:
  - `test_maillon_2c_reader_writer_consistency_after_bulk_insert_and_reopen`
- Runtime live signal after a Devenv rebuild and restart:
  - `SELECT count(*) FROM File` -> `900`
  - `SELECT count(*) FROM Chunk` -> `3370`
  - `SELECT status, count(*) FROM File GROUP BY status` -> `skipped=5`, `indexed=300`, `indexing=595`
  - `SELECT count(*) FROM EmbeddingModel` -> `2`
  - `SELECT count(*) FROM ChunkEmbedding` -> `1088`

# Elixir / Dashboard Validation State

Dashboard validation is now green under Devenv:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
- result reached first: `20 tests, 0 failures`
- result reached now: `26 tests, 0 failures`
- result reached now after VCR-5 instrumentation: `27 tests, 0 failures`
- result reached now after transient progress truth support: `30 tests, 0 failures`

Code fixes applied to reach this state:

- `CockpitLive` now subscribes to bridge events and tolerates `FileIndexed` / `ScanComplete`
- duplicate `:tick` handling was consolidated into a single runtime truth pull
- `Tracer` no longer crashes on partial or missing timestamps
- `BackpressureController` now publishes pressure guidance for the UI without pausing or scaling Oban queues

Residual non-blocking warnings still visible during `mix test`:

- runtime warnings from intentionally simulated saturation in backpressure tests
- `os_mon` shutdown noise at the end of the test VM

# Additional Work Completed On This Branch

## Operator Workflow / Validation En Conditions Reelles

Files changed:

- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/server.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/path_policy.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/batch_dispatch.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/sql_gateway.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/pool_protocol.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/pool_event_handler.ex`
- `/home/dstadel/projects/axon/src/axon-core/src/main.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/protocol.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/dispatch.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/format.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/soll.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_dx.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_governance.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_risk.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs`

What changed:

- The manual scan action exposed by the cockpit is now actually wired to the Rust-side scan path instead of being a visible no-op.
- `PARSE_BATCH` now carries a `batch_id` across Elixir and Rust.
- `BATCH_ACCEPTED` acknowledgements are now correlated instead of freeing all pending callers at once.
- watcher path policy, batch dispatch, SQL gateway access, pool protocol helpers, and pool event effects are now extracted into dedicated Elixir modules
- `server.ex` now delegates file policy and dispatch concerns instead of holding all helper logic inline
- the high-density `handle_info({:ok, path}, state)` path is now split into named private steps for project resolution, reindex decision, and routing
- extracted watcher helpers now have direct Elixir test coverage in `test/axon_nexus/axon/watcher/path_policy_test.exs` and `test/axon_nexus/axon/watcher/pool_protocol_test.exs`
- manual scan truthfulness now emits telemetry at the operator edge and at the forwarding edge:
  - `[:axon, :watcher, :manual_scan_triggered]`
  - `[:axon, :watcher, :scan_forwarded]`
  - executable coverage lives in `test/axon_nexus/axon/watcher/server_test.exs`
- `Axon.Watcher.Progress` now maintains a transient operator overlay so the cockpit can show `indexing -> live` coherently even before the next DB-derived status refresh
  - executable coverage lives in `test/axon_nexus/axon/watcher/progress_test.exs`
- `axon_query` messaging was brought back in line with actual runtime capability: structural first, semantic only when available.
- `axon_restore_soll` is now covered by tests against the official Markdown export structure.
- `mcp.rs` phase-1 refactor is complete:
  - JSON-RPC protocol types now live in `src/axon-core/src/mcp/protocol.rs`
  - SOLL export parsing and restore helper types now live in `src/axon-core/src/mcp/soll.rs`
  - `mcp.rs` remains the public entrypoint for `McpServer` and tool behavior
- `mcp.rs` phase-2 refactor is complete:
  - MCP tool catalog now lives in `src/axon-core/src/mcp/catalog.rs`
  - MCP tool dispatch now lives in `src/axon-core/src/mcp/dispatch.rs`
  - MCP table formatting helper now lives in `src/axon-core/src/mcp/format.rs`
  - public tool names and `tools/list` / `tools/call` contracts were preserved
- `mcp.rs` phase-3 is complete:
  - SOLL handlers now live in `src/axon-core/src/mcp/tools_soll.rs`
  - DX handlers now live in `src/axon-core/src/mcp/tools_dx.rs`
  - governance handlers now live in `src/axon-core/src/mcp/tools_governance.rs`
  - risk handlers now live in `src/axon-core/src/mcp/tools_risk.rs`
  - system/lattice/debug/cypher batching handlers now live in `src/axon-core/src/mcp/tools_system.rs`
  - `mcp.rs` itself is now reduced to the MCP entrypoint and module wiring
  - MCP tests were moved into `src/axon-core/src/mcp/tests.rs`
- `graph.rs` first refactor slice is complete:
  - graph-derived audit/coverage/debt/god-object helpers now live in `src/axon-core/src/graph_analytics.rs`
  - `GraphStore` public API stayed unchanged
  - `graph.rs` second refactor slice is complete:
    - query and execute primitives now live in `src/axon-core/src/graph_query.rs`
    - `GraphStore` public methods were preserved
  - `graph.rs` third refactor slice is complete:
    - ingestion persistence now lives in `src/axon-core/src/graph_ingestion.rs`
    - pending claiming, symbol embedding updates, and batch write persistence were preserved
  - `graph.rs` fourth refactor slice is complete:
    - DB bootstrap, plugin discovery, session attach, and schema creation now live in `src/axon-core/src/graph_bootstrap.rs`
  - `graph.rs` is now largely reduced to FFI type definitions and pool lifecycle
- `main.rs` first refactor slice is complete:
  - incoming telemetry command handling now lives in `src/axon-core/src/main_telemetry.rs`
  - the runtime bootstrap and socket loop remain in `main.rs`
  - command behavior and test signals were preserved
- `main.rs` second refactor slice is complete:
  - watchdog memory loop, autonomous ingestor, and initial scan startup now live in `src/axon-core/src/main_background.rs`
  - per-connection telemetry handling now also lives in `src/axon-core/src/main_telemetry.rs`
  - listener accept loop and top-level runtime wiring remain in `main.rs`
- `main.rs` third refactor slice is complete:
  - worker pool startup, semantic worker startup, and MCP HTTP startup now live in `src/axon-core/src/main_services.rs`
  - `main.rs` is now primarily runtime bootstrap + telemetry accept loop
- A dedicated incremental refactor plan now exists for the oversized MCP module:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-mcp-refactor-plan.md`
- A follow-up mapping now exists for the next core refactor candidates:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-core-refactor-candidates.md`
- A validation en conditions reelles E2E plan now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-validation-conditions-reelles-e2e.md`
- A validation en conditions reelles operational checklist now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-validation-conditions-reelles-checklist.md`
  - VCR-1 and VCR-2 now also have executable MCP coverage in `src/axon-core/src/mcp/tests.rs`
  - VCR-4 now also has executable MCP continuity coverage in `src/axon-core/src/mcp/tests.rs`
- A validation en conditions reelles run log now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-validation-conditions-reelles-log.md`
  - latest live runtime finding: `/mcp` and `/sql` are reachable after nominal bootstrap/start, but live value on Axon itself is still limited by real index coverage on some watcher/Elixir symbols
- A dedicated FOSS vectorization migration plan now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-ist-vectorization-migration-plan.md`
  - it keeps `IST` canonical, `SOLL` protected, and treats `Chunk`, `ChunkEmbedding`, `GraphProjection`, and `GraphEmbedding` as derived/versioned layers
- The first two vectorization layers are now partially implemented in Rust:
  - `Chunk` is materialized from indexed symbol spans during `insert_file_data_batch`
  - `EmbeddingModel` and `ChunkEmbedding` now exist as derived tables
  - the semantic worker now registers both symbol and chunk embedding models and can populate `ChunkEmbedding`
  - Rust validation is green after this step: `35 passed; 0 failed`
- A dedicated FOSS vectorization migration plan now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-foss-vectorization-migration-plan.md`
  - target direction is now explicitly `IST truth -> Chunk -> ChunkEmbedding -> GraphProjection -> GraphEmbedding`
- `IST` runtime compatibility is now enforced at boot in `src/axon-core/src/graph_bootstrap.rs`
  - `RuntimeMetadata` stores at least `schema_version`, `ingestion_version`, and `embedding_version`
  - when drift is detected, Axon now resets `IST` tables while preserving the `SOLL` sanctuary
- `scripts/start-v2.sh` now treats `tmux` health more strictly
- `scripts/start-v2.sh` now pre-warms Hex/Rebar non-interactively so the watcher can boot without pausing on `mix local.hex`
- `scripts/start-v2.sh` now launches the dashboard Devenv shell from the repo root before `cd src/dashboard`, avoiding false starts where `devenv.nix` was not found
- `scripts/start-v2.sh` now rejects stale release builds produced outside Devenv so runtime truth cannot silently diverge from the official build path
  - if session `axon` exists but no healthy data plane is visible, the stale session is killed and startup continues
  - local WAL and lock remnants under `.axon/graph_v2` are cleaned before relaunch

# Validation In Conditions Reelles Priority

Commercialization is no longer the immediate leading phase.

Priority order is now:

1. validation en conditions reelles on Axon itself
2. product stabilization
3. commercialization

The active intent is to make Axon genuinely useful for LLM-assisted software development and project steering before optimizing for external packaging.

# Current Git State Snapshot

Current `git status --short --branch` shows:

```text
## feat/axon-stabilization-continuation
 M .devenv/nix-eval-cache.db-shm
 M .devenv/nix-eval-cache.db-wal
 M .devenv/profile
 M .devenv/run
 M .devenv/tasks.db-shm
 M .devenv/tasks.db-wal
 M README.md
 M docs/architecture/visualize-nexus-pull.html
 M docs/working-notes/reality-first-stabilization-handoff.md
 M scripts/setup_v2.sh
 M scripts/start-v2.sh
 M scripts/stop-v2.sh
 M src/axon-core/src/graph.rs
 M src/axon-core/src/lib.rs
 M src/axon-core/src/main.rs
 M src/axon-core/src/mcp.rs
 M src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex
 M src/dashboard/lib/axon_nexus/axon/watcher/server.ex
?? .devenv/bash-bash
?? .devenv/gc/shell
?? .devenv/gc/task-config-devenv-config-task-config
?? .devenv/shell-*.sh
?? docs/plans/2026-03-30-commercial-stabilization-roadmap.md
?? docs/plans/2026-03-30-core-refactor-candidates.md
?? docs/plans/2026-03-30-mcp-refactor-plan.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_*.md
?? src/axon-core/src/graph_analytics.rs
?? src/axon-core/src/graph_bootstrap.rs
?? src/axon-core/src/graph_ingestion.rs
?? src/axon-core/src/graph_query.rs
?? src/axon-core/src/main_background.rs
?? src/axon-core/src/main_services.rs
?? src/axon-core/src/main_telemetry.rs
?? src/axon-core/src/mcp/
```

Interpretation of the current snapshot:

- `.devenv/*` changes are mostly runtime artifacts from Devenv execution.
- multiple `SOLL_EXPORT_*.md` files are considered legitimate historical exports of the `SOLL` conceptual layer and are intentionally kept.
- HydraDB should now be considered detached from the active Devenv workflow unless explicitly reintroduced later.

Re-check current Git state before acting.

# Resume Checklist

When resuming after compaction, do this in order:

1. Read this file completely.
2. Run `git status --short --branch`.
3. Run `git diff --stat`.
4. Re-check the branch is still `feat/axon-stabilization-continuation`.
5. Re-run Rust validation:
   - `cd src/axon-core && cargo test`
6. Continue the interrupted dashboard validation:
   - `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
7. If both are green, continue with:
   - operator workflow truthfulness
   - MCP usefulness for LLM development
   - SOLL export / restore reliability
   - progressive refactoring of the dashboard watcher layer

# Recommended Next Steps

Primary next step:

1. keep `cargo test` green after the completed `mcp.rs` split
2. continue refactoring `server.ex` and `pool_facade.ex` by responsibility without changing operator-visible behavior
3. execute the new validation en conditions reelles checklist on Axon itself and record evidence
4. continue improving MCP usefulness for LLM-assisted development

Secondary next step:

5. keep aligning dashboard actions and MCP outputs with real value for LLM-assisted development

Method skill already created:

- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`

# Anti-Drift Rule

After compaction, do not trust any summary blindly, including this one.

Use this file as a map, then verify the code and runtime state directly.
