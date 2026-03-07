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
      {:jason, "~> 1.4"}, # Gardé pour du debug ou de la config si besoin
      {:msgpax, "~> 2.4"}, # Pour le transport binaire ultra-rapide
      {:nimble_pool, "~> 1.0"}, # Pour le pool de workers Python
      {:file_system, "~> 1.0"} # Pour la surveillance native de l'OS
    ]
  end
end
