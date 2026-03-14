import Config

config :axon_watcher,
  ecto_repos: [Axon.Watcher.Repo]

config :axon_watcher, Axon.Watcher.Endpoint,
  http: [port: String.to_integer(System.get_env("PHOENIX_PORT") || "6061")],
  adapter: Bandit.PhoenixAdapter,
  server: true,
  check_origin: false,
  live_view: [signing_salt: "axon_cockpit_salt"],
  secret_key_base: "uT+pL/Uv67tW4K1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1Z1"

config :axon_watcher, Axon.Watcher.Repo,
  database: Path.join(System.user_home!(), ".axon/runtime/oban.db"),
  pool_size: 5

config :axon_watcher, Oban,
  repo: Axon.Watcher.Repo,
  engine: Oban.Engines.Lite,
  plugins: [Oban.Plugins.Pruner],
  queues: [
    indexing_critical: [limit: 10],
    indexing_hot: [limit: 5],
    indexing_default: [limit: 10]
  ]

config :phoenix, :json_library, Jason
