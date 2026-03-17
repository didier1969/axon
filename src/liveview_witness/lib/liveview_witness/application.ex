defmodule LiveView.Witness.Application do
  @moduledoc false
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      {Registry, keys: :unique, name: LiveView.Witness.Registry},
      {Phoenix.PubSub, name: LiveView.Witness.PubSub}
    ]

    opts = [strategy: :one_for_one, name: LiveView.Witness.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
