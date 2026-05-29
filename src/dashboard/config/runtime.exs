import Config

# config/runtime.exs is executed for all environments, including
# during releases. It is executed after compilation and before the
# system starts, so it is typically used to load production configuration
# and secrets from environment variables or elsewhere.
#
# REQ-AXO-901802 (MIL-AXO-028 cat B) — single source of config truth.
# All AXON_* env vars are read HERE only. Consumers in lib/ MUST read
# via Application.get_env(:axon_dashboard, KEY) — no scattered
# System.get_env in modules.

# ## Phoenix release server toggle
if System.get_env("PHX_SERVER") do
  config :axon_dashboard, AxonDashboardWeb.Endpoint, server: true
end

# REQ-AXO-901654 — only override endpoint port for non-test environments.
# config/test.exs binds the Wallaby endpoint to 44126; an unconditional
# override here re-binds it and collides with the running dashboard.
if config_env() != :test do
  config :axon_dashboard, AxonDashboardWeb.Endpoint,
    http: [port: String.to_integer(System.get_env("PHX_PORT") || System.get_env("PORT") || "44127")]
end

# REQ-AXO-901802 + REQ-AXO-901799 + REQ-AXO-901800 — instance-aware config
# centralization. Replaces scattered System.get_env in lib/ with a single
# Application.env populated here based on AXON_INSTANCE_KIND.
if config_env() != :test do
  instance_kind = System.get_env("AXON_INSTANCE_KIND") || "live"

  unless instance_kind in ["dev", "live"] do
    raise "Unknown AXON_INSTANCE_KIND=#{inspect(instance_kind)}. Expected 'dev' or 'live'."
  end

  {default_brain_port, default_telemetry_socket} =
    case instance_kind do
      "dev" -> {44139, "/tmp/axon-dev-indexer-telemetry.sock"}
      "live" -> {44129, "/tmp/axon-live-indexer-telemetry.sock"}
    end

  default_sql_url = "http://127.0.0.1:#{default_brain_port}/sql"
  default_mcp_endpoint = "http://127.0.0.1:#{default_brain_port}/mcp"

  config :axon_dashboard, :instance_kind, instance_kind

  config :axon_dashboard, Axon.Watcher.SqlGateway,
    url: System.get_env("AXON_SQL_URL") || default_sql_url,
    allow_cross_instance_fallback: false

  config :axon_dashboard, Axon.Watcher.McpClient,
    endpoint: System.get_env("AXON_MCP_ENDPOINT") || default_mcp_endpoint

  config :axon_dashboard, AxonDashboard.BridgeClient,
    telemetry_socket_path: default_telemetry_socket

  # REQ-AXO-901802 (MIL-AXO-028 cat B) — Application.env-driven workspace
  # root replaces ad-hoc `File.cwd!()` walks in display helpers (e.g.
  # Axon.Watcher.Telemetry.get_top_dir/1). DEVENV_ROOT is set by the
  # devenv shell ; falls back to "/" so production deployments without
  # devenv still work without raising on missing-env.
  config :axon_dashboard,
         :workspace_root,
         System.get_env("DEVENV_ROOT") || "/"
end

# Prod-specific configuration (secrets, host)
if config_env() == :prod do
  # The secret key base is used to sign/encrypt cookies and other secrets.
  secret_key_base =
    System.get_env("SECRET_KEY_BASE") ||
      raise """
      environment variable SECRET_KEY_BASE is missing.
      You can generate one by calling: mix phx.gen.secret
      """

  host = System.get_env("PHX_HOST") || "example.com"

  config :axon_dashboard, :dns_cluster_query, System.get_env("DNS_CLUSTER_QUERY")

  config :axon_dashboard, AxonDashboardWeb.Endpoint,
    url: [host: host, port: 443, scheme: "https"],
    http: [
      # Enable IPv6 and bind on all interfaces.
      ip: {0, 0, 0, 0, 0, 0, 0, 0}
    ],
    secret_key_base: secret_key_base
end
