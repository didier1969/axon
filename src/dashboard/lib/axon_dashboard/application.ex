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
      # REQ-AXO-901803 (MIL-AXO-028 cat C) — supervised Task pool so
      # LiveView fire-and-forget work (catalog fetch, async data
      # hydration) gets graceful shutdown + crash logging instead of
      # leaking via bare `Task.start/1`.
      {Task.Supervisor, name: AxonDashboard.TaskSupervisor},
      # REQ-AXO-901806 F6 — IndexerHeartbeat + McpPoller GenServers retired.
      # Single source of truth = `{:dashboard_state, state}` broadcast by
      # BridgeClient from the brain's 1 Hz dashboard_state_v1 event.
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
