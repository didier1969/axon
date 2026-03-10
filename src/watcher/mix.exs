defmodule Axon.Watcher.MixProject do
  use Mix.Project

  def project do
    [
      app: :axon_watcher,
      version: "1.0.0",
      elixir: "~> 1.14",
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  def application do
    [
      extra_applications: [:logger],
      mod: {Axon.Watcher.Application, []}
    ]
  end

  defp deps do
    [
      {:rustler, "~> 0.34.0"},
      {:jason, "~> 1.4"},
      {:msgpax, "~> 2.4"},
      {:nimble_pool, "~> 1.0"},
      {:file_system, "~> 1.0"},
      {:phoenix, "~> 1.7"},
      {:phoenix_live_view, "~> 1.0"},
      {:bandit, "~> 1.0"},
      {:ecto, "~> 3.10"},
      {:ecto_sqlite3, "~> 0.10"},
      {:oban, "~> 2.18"}
    ]
  end
end
