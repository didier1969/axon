defmodule AxonDashboard.Application do
  # See https://hexdocs.pm/elixir/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    # 1. Start Erlang Clustering with Watcher
    Node.connect(:"watcher@127.0.0.1")
    
    children = [
      AxonDashboardWeb.Telemetry,
      {DNSCluster, query: Application.get_env(:axon_dashboard, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: AxonDashboard.PubSub},
      # Start a worker by calling: AxonDashboard.Worker.start_link(arg)
      # {AxonDashboard.Worker, arg},
      AxonDashboard.BridgeClient,
      # Start to serve requests, typically the last entry
      AxonDashboardWeb.Endpoint
    ]

    # See https://hexdocs.pm/elixir/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: AxonDashboard.Supervisor]
    Supervisor.start_link(children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    AxonDashboardWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
