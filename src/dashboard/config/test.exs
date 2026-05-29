import Config

# REQ-AXO-901649 — Wallaby E2E feature tests need the endpoint actually
# serving on a real HTTP port (not just `server: false`). We use port
# 44126 — distinct from dev/live cockpit (44127) and HYDRA (44128..44132)
# so a running brain doesn't clash with the test instance.
config :axon_dashboard, AxonDashboardWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 44126],
  secret_key_base: "Gr1hatC1TQcyEW5avHIisHGjCj47ubL8Ps3Oq94YnqQ2UnFW/NN9v3G9VKsuq23j",
  server: true,
  # REQ-AXO-901683 — Wallaby drives Chromium against http://127.0.0.1:44126
  # but Phoenix.Socket's default `check_origin` rejects mismatched hosts,
  # blocking the LiveView WebSocket. With the socket closed, the cockpit
  # JS displays the `.phx-error` banner (errors_test.exs:20 fails) and
  # every subsequent feature test starves waiting for LiveView-rendered
  # content. `check_origin: false` is safe here because the endpoint only
  # binds to 127.0.0.1 in the test env (see :http :ip above).
  check_origin: false

# Print only warnings and errors during test
config :logger, level: :warning

# Initialize plugs at runtime for faster test compilation
config :phoenix, :plug_init_mode, :runtime

# Enable helpful, but potentially expensive runtime checks
config :phoenix_live_view,
  enable_expensive_runtime_checks: true

# Sort query params output of verified routes for robust url comparisons
config :phoenix,
  sort_verified_routes_query_params: true

config :axon_dashboard, telemetry_socket_path: "/tmp/axon-telemetry-test.sock"

# REQ-AXO-901802 (MIL-AXO-028 cat B) — test-mode Application.env populates
# the same keys as config/runtime.exs would in dev/live. Single source
# of truth across env: every consumer reads via Application.get_env.
config :axon_dashboard, :instance_kind, "test"

config :axon_dashboard, Axon.Watcher.SqlGateway,
  url: "http://127.0.0.1:1/sql",
  allow_cross_instance_fallback: false

config :axon_dashboard, AxonDashboard.BridgeClient,
  telemetry_socket_path: "/tmp/axon-telemetry-test.sock"

# REQ-AXO-901649 — Wallaby driver configuration. Headless Chrome + small
# viewport so the suite runs identically on a developer workstation and on
# a headless CI box. ChromeDriver binary is resolved from $PATH (devenv.nix
# provisions it via pkgs.chromedriver + pkgs.chromium).
config :wallaby,
  driver: Wallaby.Chrome,
  chromedriver: [
    headless: true
  ],
  base_url: "http://127.0.0.1:44126",
  screenshot_on_failure: true,
  screenshot_dir: "tmp/wallaby_screenshots"

# REQ-AXO-901649 + REQ-AXO-901802 — point the MCP client at a closed port
# so McpClient fails fast in tests instead of hanging on the live brain.
# McpLive handles the error path gracefully (`{:tools_error, _reason}` →
# loaded? = true, empty list), and that's exactly what the McpLive feature
# tests assert when they stub tools via direct LiveView pid injection.
config :axon_dashboard, Axon.Watcher.McpClient,
  endpoint: "http://127.0.0.1:1/mcp"

# REQ-AXO-901649 + REQ-AXO-901802 — MCP catalog fixture path for Wallaby
# hermetic tests. Read from env var AXON_MCP_FIXTURE_PATH at compile time
# so the per-test setup can wire it in via Mix.env() / System.put_env.
if path = System.get_env("AXON_MCP_FIXTURE_PATH") do
  config :axon_dashboard, :mcp_fixture_path, path
end
