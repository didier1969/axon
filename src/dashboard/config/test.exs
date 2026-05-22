import Config

# REQ-AXO-901649 — Wallaby E2E feature tests need the endpoint actually
# serving on a real HTTP port (not just `server: false`). We use port
# 44126 — distinct from dev/live cockpit (44127) and HYDRA (44128..44132)
# so a running brain doesn't clash with the test instance.
config :axon_dashboard, AxonDashboardWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 44126],
  secret_key_base: "Gr1hatC1TQcyEW5avHIisHGjCj47ubL8Ps3Oq94YnqQ2UnFW/NN9v3G9VKsuq23j",
  server: true

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

# REQ-AXO-901649 — point the MCP client at a closed port so McpClient
# fails fast in tests instead of hanging on the live brain (which the
# operator MUST NOT have torn down for tests to run). McpLive handles the
# error path gracefully (`{:tools_error, _reason}` → loaded? = true,
# empty list), and that's exactly what the McpLive feature tests assert
# when they stub tools via direct LiveView pid injection.
config :axon_dashboard, mcp_endpoint: "http://127.0.0.1:1/mcp"
