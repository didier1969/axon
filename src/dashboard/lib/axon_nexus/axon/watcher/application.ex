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

    children = visualization_children()

    opts = [strategy: :rest_for_one, name: Axon.Watcher.Supervisor]
    Supervisor.start_link(children, opts)
  end

  def visualization_children do
    [
      Axon.Watcher.Repo,
      Axon.Watcher.Telemetry,
      Axon.Watcher.Tracer,
      Axon.Watcher.PoolFacade,
      Axon.Watcher.TrafficGuardian,
      {Phoenix.PubSub, name: Axon.PubSub},
      Axon.Watcher.Endpoint
    ]
  end
end
