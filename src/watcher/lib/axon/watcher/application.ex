defmodule Axon.Watcher.Application do
  @moduledoc """
  The entry point for the Axon Watcher application.
  Sets up the supervision tree for Pod A.
  """
  use Application

  @impl true
  def start(_type, _args) do
    # On force le port pour le Cockpit
    System.put_env("PHOENIX_PORT", "6061")

    children = [
      Axon.Watcher.Repo,
      Axon.Watcher.Telemetry,
      {Oban, Application.fetch_env!(:axon_watcher, Oban)},
      {Axon.Watcher.Server, []},
      Axon.Watcher.Endpoint
    ]

    opts = [strategy: :one_for_one, name: Axon.Watcher.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
