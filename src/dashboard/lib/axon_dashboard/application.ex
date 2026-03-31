defmodule AxonDashboard.Application do
  # See https://hexdocs.pm/elixir/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      AxonDashboardWeb.Telemetry,
      {Phoenix.PubSub, name: AxonDashboard.PubSub},
      Axon.Watcher.Tracer,
      Axon.Watcher.Telemetry,
      AxonDashboard.TelemetryHandler,
      Axon.Watcher.Repo,
      Axon.Watcher.StatsCache,
      Axon.Watcher.Auditor,
      Axon.Watcher.PoolFacade,
      Axon.Watcher.TrafficGuardian,
      Axon.ResourceMonitor,
      Axon.BackpressureController,
      {DNSCluster, query: Application.get_env(:axon_dashboard, :dns_cluster_query) || :ignore},
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
