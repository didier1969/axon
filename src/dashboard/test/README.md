# Dashboard test suite

REQ-AXO-901649 — three layers:

| Layer | Path | Runner | Notes |
| --- | --- | --- | --- |
| Unit / ConnCase | `test/axon_dashboard_web/{controllers,live}/**` | `mix test` | Default ExUnit run. |
| Feature (Wallaby + Chrome) | `test/axon_dashboard_web/features/**` | `mix test --only feature` OR `bash scripts/test-dashboard-e2e.sh` | Requires devenv shell (chromedriver + chromium). |
| Auxiliary watcher tests | `test/axon_nexus/**` | `mix test` | Run as part of unit layer. |

## Quick start

```bash
# Inside devenv shell (mandatory — chromedriver lives in /nix/store)
cd src/dashboard
mix deps.get
mix test --only feature
```

Or use the wrapper, which auto-enters `devenv shell` and runs the
preflight (chromedriver + chromium on PATH) before invoking mix:

```bash
bash scripts/test-dashboard-e2e.sh
```

## Stubs (no live brain required)

The feature suite is hermetic. `test/test_helper.exs` flips three env
vars so the dashboard runs against fixtures rather than the live brain:

| Env var | Value | Effect |
| --- | --- | --- |
| `AXON_MCP_ENDPOINT` | `http://127.0.0.1:1/mcp` | McpClient fails fast (no hanging RPCs). |
| `AXON_INDEXER_HEARTBEAT_PATH` | `/tmp/axon-test-heartbeat-missing.json` | IndexerHeartbeat broadcasts `:missing`. |
| `AXON_MCP_FIXTURE_PATH` | `test/support/fixtures/mcp_tools.json` | McpLive reads a 68-tool JSON fixture. |

## Wallaby driver

ChromeDriver + Chromium 146 are pinned through `devenv.nix`
(`pkgs.chromedriver` + `pkgs.chromium`). Headless mode is the default
(see `config/test.exs`).

## Coverage map (REQ-AXO-901649)

| File | Page / contract |
| --- | --- |
| `features/nav_test.exs` | Shell chrome on every route + active link highlight + nav routing. |
| `features/pipeline_test.exs` | CPT-AXO-054 topology surface (A1..B3), KPI cards, worker config table, GPU panel. |
| `features/projects_test.exs` | Projects table headers + totals + sort buttons + footer cadence + no crash banner. |
| `features/mcp_test.exs` | 68-tool catalog, category tabs, filter narrowing, no-match message, empty-filter regression (REQ-AXO-901649). |
| `features/errors_test.exs` | `.phx-error` absence on every route + REQ-AXO-901648 stale-gzip regression guard + static-asset 200/304. |
