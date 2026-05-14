# Python Operational Audit

Date: `2026-03-31`
Branch: `feat/rust-first-control-plane`

## Scope

This audit classifies Python artifacts by operational relevance to the current Axon runtime.

Canonical startup path considered:

- `scripts/setup_v2.sh`
- `scripts/start-v2.sh`
- `scripts/stop-v2.sh`
- `devenv.nix`

## Classification

### Current

- `src/axon-core/src/parser/python_bridge/datalog_parser.py`
- `src/axon-core/src/parser/python_bridge/typeql_parser.py`

Why:

- these two scripts are still called by the Rust runtime parser bridges for Datalog and TypeQL
- they are the only Python code found on a real runtime path

### Tolerated

Artifacts not on the canonical startup path, but still plausible as diagnostics, migration helpers, or transitional tests:

- `scripts/benchmark_mcp.py`
- `scripts/check_nexus_health.py`
- `scripts/debug_graph.py`
- `scripts/diagnose_cockpit.py`
- `scripts/e2e_mcp_test.py`
- `scripts/get_db_stats.py`
- `scripts/inject_real_soll.py`
- `scripts/mcp_verify.py`
- `scripts/mcp_verify_360.py`
- `scripts/monitor_indexing.py`
- `scripts/test_all_mcp_commands.py`
- `scripts/axon_duckdb_restore.py`
- `scripts/axon_pillar_restore.py`
- `scripts/axon_pillar_sync.py`
- `scripts/axon_soll_restore.py`
- `scripts/axon_nuclear_console.py`
- `tests/e2e_pipeline_test.py`
- `tests/simulate_watcher.py`
- `tests/test_db_init.py`
- `tests/test_elixir_extractor.py`
- `tests/verify_ingestion_rigorous.py`
- `tests/verify_soll_isolation.py`
- `benchmarks/run_benchmark.py`
- `src/axon-core/benchmark.py`

Reasoning:

- not part of the canonical runtime
- some still target legacy sockets or removed MCP contracts
- they should be reviewed again later, but not removed blindly in this first slice

### Obsolete

High-confidence obsolete artifacts removed in this slice:

- `scripts/mcp-stdio-proxy.py`
- `scripts/demo_uds.py`
- `convert_tests.py`
- `convert_tests2.py`
- `convert_tests3.py`
- `go_lang.py`
- `run_audit_benchmark.py`
- `test_attach.py`
- `test_mcp_init.py`
- `test_perf.py`
- `test_small.py`

Why:

- not used by the canonical startup path
- not required by the Rust runtime
- rooted in old stdio / UDS / ad hoc conversion flows
- no trustworthy operational role in the current Rust-first architecture

## Runtime Conclusion

Python is still required for now, but only for:

- `python3`
- `datalog_parser.py`
- `typeql_parser.py`

Everything else should be treated as optional tooling until explicitly revalidated.
