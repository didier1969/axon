defmodule Axon.Watcher.Application do
  @moduledoc """
  The entry point for the Axon Watcher application.
  Sets up the supervision tree for Pod A.
  """
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      {PartitionSupervisor, child_spec: Axon.Watcher.Worker, name: Axon.Watcher.WorkerPool},
      {Axon.Watcher.Server, []}
    ]

    opts = [strategy: :one_for_one, name: Axon.Watcher.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
