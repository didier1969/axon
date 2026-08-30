ExUnit.start()

# REQ-AXO-901649 — Wallaby setup for E2E feature tests. ChromeDriver is
# provisioned by devenv.nix. Wallaby is configured via :wallaby config in
# config/test.exs.
{:ok, _} = Application.ensure_all_started(:wallaby)

# Force the MCP client to a closed port so the suite never depends on a
# running brain. McpClient reads AXON_MCP_ENDPOINT lazily at every call,
# so updating the env var here is sufficient.
System.put_env("AXON_MCP_ENDPOINT", "http://127.0.0.1:1/mcp")

# REQ-AXO-901649 — stub the MCP catalog from a JSON fixture so McpLive
# tests assert the 68-tool surface without a live brain.
fixture_path =
  Path.expand("support/fixtures/mcp_tools.json", __DIR__)
  |> Path.absname()

System.put_env("AXON_MCP_FIXTURE_PATH", fixture_path)

# REQ-AXO-902570 — et le poser AUSSI dans l'Application env. `config/test.exs`
# lit `AXON_MCP_FIXTURE_PATH` à la COMPILATION, or on est ici au runtime : le
# `config :axon_dashboard, :mcp_fixture_path` n'était donc jamais posé, et
# `McpLive.initial_tools/0` — qui lit cette clé, pas la variable d'environnement
# — rendait `{[], false, nil}`, soit « 0 public tools » sur toute la suite.
Application.put_env(:axon_dashboard, :mcp_fixture_path, fixture_path)
