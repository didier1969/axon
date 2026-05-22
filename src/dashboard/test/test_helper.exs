ExUnit.start()

# REQ-AXO-901649 — Wallaby setup for E2E feature tests. ChromeDriver is
# provisioned by devenv.nix. Wallaby is configured via :wallaby config in
# config/test.exs.
{:ok, _} = Application.ensure_all_started(:wallaby)

# Force the MCP client to a closed port so the suite never depends on a
# running brain. McpClient reads AXON_MCP_ENDPOINT lazily at every call,
# so updating the env var here is sufficient.
System.put_env("AXON_MCP_ENDPOINT", "http://127.0.0.1:1/mcp")

# Point IndexerHeartbeat at a guaranteed-missing path so it broadcasts
# `:indexer_heartbeat_missing` instead of reading the live indexer's
# on-disk runtime file. The Pipeline LiveView already handles the
# "missing" state.
System.put_env("AXON_INDEXER_HEARTBEAT_PATH", "/tmp/axon-test-heartbeat-missing.json")

# REQ-AXO-901649 — stub the MCP catalog from a JSON fixture so McpLive
# tests assert the 68-tool surface without a live brain.
fixture_path =
  Path.expand("support/fixtures/mcp_tools.json", __DIR__)
  |> Path.absname()

System.put_env("AXON_MCP_FIXTURE_PATH", fixture_path)
