# Indexer Project Discovery Decoupling Plan

1. Replace `SOLL`-backed project discovery in `main_background.rs` with local canonical discovery via `project_meta::discover_project_identities()`.
2. Add helper tests for the local orchestration discovery path.
3. Run targeted Rust tests for the orchestrator helper.
4. Re-run `reset-dev-baseline.sh`.
5. Re-run `qualify-dev-cold.sh`.
6. Validate:
   - `indexer` no longer logs `SOLL` lock conflicts for orchestration discovery
   - cold run produces non-zero discovery/backlog if eligible files are present.
