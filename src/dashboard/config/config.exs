# This file is responsible for configuring your application
# and its dependencies with the aid of the Config module.
#
# This configuration file is loaded before any dependency and
# is restricted to this project.

# General application configuration
import Config

config :axon_dashboard,
  ecto_repos: [Axon.Watcher.Repo],
  generators: [timestamp_type: :utc_datetime]

config :axon_dashboard, Axon.Watcher.Repo,
  adapter: Ecto.Adapters.SQLite3,
  database: "axon_nexus.db",
  pool_size: 1, # SERIALIZED WRITES to avoid 'Database busy'
  journal_mode: :wal,
  busy_timeout: 5000,
  synchronous: :normal # High performance mode for WAL

config :axon_dashboard, Oban,
  repo: Axon.Watcher.Repo,
  engine: Oban.Engines.Lite,
  # Prune less frequently to avoid lock contention during massive ingestion
  plugins: [{Oban.Plugins.Pruner, interval: 300_000}], 
  queues: [
    indexing_critical: [limit: 10],
    indexing_hot: [limit: 5],
    indexing_default: [limit: 10],
    # Strict single-thread isolation for massive >1MB files
    indexing_titan: [limit: 1]
  ]

config :axon_dashboard, Axon.BackpressureController,
  cpu_hard_limit: 70.0,
  ram_hard_limit: 70.0,
  io_hard_limit: 20.0

# Configure the endpoint
config :axon_dashboard, AxonDashboardWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [html: AxonDashboardWeb.ErrorHTML, json: AxonDashboardWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: AxonDashboard.PubSub,
  live_view: [signing_salt: "NFKoFzUv"]

# Configure esbuild (the version is required)
config :esbuild,
  version: "0.25.4",
  axon_dashboard: [
    args:
      ~w(js/app.js --bundle --target=es2022 --outdir=../priv/static/assets/js --external:/fonts/* --external:/images/* --alias:@=.),
    cd: Path.expand("../assets", __DIR__),
    env: %{"NODE_PATH" => [Path.expand("../deps", __DIR__), Mix.Project.build_path()]}
  ]

# Configure tailwind (the version is required)
config :tailwind,
  version: "4.1.12",
  axon_dashboard: [
    args: ~w(
      --input=assets/css/app.css
      --output=priv/static/assets/css/app.css
    ),
    cd: Path.expand("..", __DIR__)
  ]

# Configure Elixir's Logger
config :logger, :default_formatter,
  format: "[$date $time] $metadata[$level] $message\n",
  metadata: [:request_id]

# Use Jason for JSON parsing in Phoenix
config :phoenix, :json_library, Jason

# Import environment specific config. This must remain at the bottom
# of this file so it overrides the configuration defined above.
import_config "#{config_env()}.exs"
