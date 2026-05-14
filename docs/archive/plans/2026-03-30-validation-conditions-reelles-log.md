---
title: Validation En Conditions Reelles Log
date: 2026-03-30
status: active
branch: feat/axon-stabilization-continuation
---

# Run 1

- Date: 2026-03-30
- Repo: Axon
- Scenario: `VCR-1 Symbol Discovery`
- Prompt: `Where is the scan trigger wired?`
- Baseline:
  - likely manual path is `rg "trigger_scan|trigger_global_scan"` then open watcher and pool files
  - usable, but still requires repo-specific intuition
- Axon:
  - executable MCP coverage added in `src/axon-core/src/mcp/tests.rs::test_vcr1_symbol_discovery_for_scan_trigger_flow`
  - first run failed because `axon_query` handled the natural phrase `trigger scan` too literally
  - tool was then improved to normalize spaces, separators, and compacted symbol names in `src/axon-core/src/mcp/tools_dx.rs`
  - current result is green in `cargo test`
- Score: `2`
- Follow-up:
  - still validate this against the live indexed Axon repository, not only an in-memory graph fixture
  - compare against a real MCP call through `/mcp`, not only direct in-process request handling

# Run 2

- Date: 2026-03-30
- Repo: Axon
- Scenario: `VCR-2 Impact Before Change`
- Prompt: `What breaks if parse_batch ACK semantics change?`
- Baseline:
  - manual path is to inspect callers, batch flow, and protocol edges through `rg` + file reading
  - accurate but slower and more fragile under broader protocol changes
- Axon:
  - executable MCP coverage added in `src/axon-core/src/mcp/tests.rs::test_vcr2_impact_before_change_on_public_api`
  - first run failed because `axon_impact` assumed an internal symbol id instead of a human symbol name
  - tool was then corrected in `src/axon-core/src/mcp/tools_risk.rs` to resolve by symbol name or id
  - API break analysis already produced useful consumer output
  - current result is green in `cargo test`
- Score: `2`
- Follow-up:
  - verify impact usefulness on a denser real slice of Axon, especially the watcher/Rust bridge path
  - add a live scenario for `axon_bidi_trace` to complement the impact report

# Current Interpretation

The first two validation runs are not yet full terrain validation, but they already prove something important:

- when a scenario failed, the failure corresponded to a real product weakness
- the fix was made in the tool surface itself, not in the test expectation
- the result is now executable and repeatable

That is the right direction for Axon.

# Run 3

- Date: 2026-03-30
- Repo: Axon
- Scenario: `VCR-5 Operator Truthfulness`
- Prompt: `Does the visible manual scan action actually propagate to the data plane?`
- Baseline:
  - manual verification requires clicking the UI and then inspecting logs or runtime state
  - this is possible, but it is weakly structured and easy to misread
- Axon:
  - watcher operator path now emits `[:axon, :watcher, :manual_scan_triggered]`
  - bridge forwarding path now emits `[:axon, :watcher, :scan_forwarded]`
  - executable coverage added in `src/dashboard/test/axon_nexus/axon/watcher/server_test.exs`
  - `Progress` now preserves transient operator truth through `update_status/2`, so `indexing -> live` is visible even when the SQL gateway is not the immediate source of truth
  - executable coverage added in `src/dashboard/test/axon_nexus/axon/watcher/progress_test.exs`
  - `mix test` is green with this path included
- Score: `2`
- Follow-up:
  - still need a live run with the real Rust data plane connected
  - next useful step is to bind forwarded scan evidence and progress movement into a compact operator-visible audit trail

# Run 4

- Date: 2026-03-30
- Repo: Axon
- Scenario: `VCR-4 SOLL Continuity`
- Prompt: `Can SOLL survive a create -> export -> restore cycle without losing conceptual continuity?`
- Baseline:
  - manual continuity is fragile and expensive
  - rebuilding conceptual state from documents by hand is slow and error-prone
- Axon:
  - executable MCP coverage added in `src/axon-core/src/mcp/tests.rs::test_vcr4_soll_continuity_create_export_restore_verify`
  - the scenario creates conceptual entities through `axon_soll_manager`
  - exports them through `axon_export_soll`
  - restores them in a fresh store through `axon_restore_soll`
  - verifies restored counts for Vision, Pillar, Concept, Milestone, Requirement, Decision, and Validation
  - current result is green in `cargo test`
- Score: `2`
- Follow-up:
  - still validate the same continuity path against the real Axon repository state, not only a fresh in-memory graph
  - add a live verification of restored usability through subsequent `axon_query` / `axon_inspect` style project steering questions

# Run 5

- Date: 2026-03-30
- Repo: Axon
- Scenario: `Live MCP Runtime Check`
- Prompt: `Does the live /mcp surface work on Axon itself, and does it already provide useful answers on the real index?`
- Baseline:
  - the nominal scripts should bootstrap, build, and start the runtime without ad hoc commands
  - `/mcp` and `/sql` should answer on the running data plane
- Axon:
  - `scripts/setup_v2.sh` initially failed because it copied `axon-core` from the wrong build path
  - this exposed a real reproducibility defect: `CARGO_TARGET_DIR` was redirected by Devenv, but the script still assumed `src/axon-core/target/release`
  - the script was corrected, and the nominal bootstrap completed successfully
  - the nominal start path now brings up the data plane and exposes live `/mcp` and `/sql`
  - confirmed live:
    - `tools/list` works over HTTP
    - `SELECT count(*) FROM File` returns a real indexed volume
  - current live limitation:
    - `axon_query(\"trigger scan\")` returns no result on the real Axon index
    - `axon_impact(\"parse_batch\")` returns no impact on the real Axon index
    - this points to a real indexing/extraction gap on the live Axon repository slice, not just a presentation issue
- Score: `1`
- Follow-up:
  - investigate live symbol extraction coverage for Elixir watcher modules
  - confirm whether these files are absent from the current watched root or present without the expected symbol extraction
  - only after that re-score `VCR-1` and `VCR-2` in true live conditions
