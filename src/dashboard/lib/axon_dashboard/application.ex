# Copyright (c) Didier Stadelmann. All rights reserved.

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
      {DNSCluster, query: Application.get_env(:axon_dashboard, :dns_cluster_query) || :ignore},
      # REQ-AXO-901647: dashboard rebuild — pipeline cockpit needs a 1Hz heartbeat
      # broadcast over `bridge_events` so PipelineLive can push_event to the JS
      # SVG hook without polling.
      Axon.Watcher.IndexerHeartbeat,
      # REQ-AXO-901647: poll MCP status verbose every 30s for the catalog page
      # and embedding_status every 5s for the cockpit's worker config snapshot.
      Axon.Watcher.McpPoller,
      AxonDashboard.BridgeClient,
      # REQ-AXO-094 — install the BEAM alarm relay AFTER BridgeClient
      # so a `:set_alarm` fired during startup has a connected (or
      # connecting) socket to push through.
      AxonDashboard.BeamAlarmReporter,
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
