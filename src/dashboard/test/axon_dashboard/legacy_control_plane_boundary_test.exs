# Copyright (c) Didier Stadelmann. All rights reserved.
defmodule AxonDashboard.LegacyControlPlaneBoundaryTest do
  use ExUnit.Case, async: true

  test "dashboard no longer configures Oban as a canonical ingestion queue" do
    assert Application.get_env(:axon_dashboard, Oban) == nil
  end

  test "dashboard bridge no longer exposes runtime command surface" do
    refute function_exported?(AxonDashboard.BridgeClient, :trigger_scan, 0)
    refute function_exported?(AxonDashboard.BridgeClient, :trigger_scan, 1)
    refute function_exported?(AxonDashboard.BridgeClient, :stop_scan, 0)
    refute function_exported?(AxonDashboard.BridgeClient, :reset_db, 0)
    refute function_exported?(AxonDashboard.BridgeClient, :trigger_async_audit, 1)
  end

  test "pool protocol no longer exposes legacy batch acknowledgements" do
    refute function_exported?(Axon.Watcher.PoolProtocol, :ack_targets, 2)
  end

  test "progress dashboard does not expose local mutable overlays anymore" do
    refute function_exported?(Axon.Watcher.Progress, :update_status, 2)
    refute function_exported?(Axon.Watcher.Progress, :purge_repo, 1)
  end

  test "dead read-side legacy modules are no longer compiled into the dashboard" do
    assert :non_existing == :code.which(Axon.Watcher.PoolFacade)
    assert :non_existing == :code.which(Axon.BackpressureController)
    assert :non_existing == :code.which(Axon.ResourceMonitor)
    assert :non_existing == :code.which(AxonDashboard.TelemetryHandler)
    assert :non_existing == :code.which(AxonDashboardWeb.StatusLive)
    assert :non_existing == :code.which(Axon.Watcher.StatsCache)
    assert :non_existing == :code.which(Axon.Watcher.PoolEventHandler)
    assert :non_existing == :code.which(Axon.Watcher.Auditor)
    assert :non_existing == :code.which(Axon.Watcher.Tracking)
    assert :non_existing == :code.which(Axon.Watcher.IndexedProject)
    assert :non_existing == :code.which(Axon.Watcher.IndexedFile)
  end

  # REQ-AXO-901801 (MIL-AXO-028 cat A) — session 60 cleanup:
  # supervision tree dupliquée, SQLite Repo orphelin, Endpoint dans router.ex,
  # CockpitLive unreachable, Schemas/Progress/ProjectMetrics dead.
  test "MIL-AXO-028 cat A — Watcher.Application + Repo + Endpoint + cockpit subtree removed" do
    assert :non_existing == :code.which(Axon.Watcher.Application)
    assert :non_existing == :code.which(Axon.Watcher.Repo)
    assert :non_existing == :code.which(Axon.Watcher.Router)
    assert :non_existing == :code.which(Axon.Watcher.Endpoint)
    assert :non_existing == :code.which(Axon.Watcher.FaviconController)
    assert :non_existing == :code.which(Axon.Watcher.CockpitLive)
    assert :non_existing == :code.which(Axon.Watcher.Schemas.Symbol)
    assert :non_existing == :code.which(Axon.Watcher.Schemas.Relationship)
    assert :non_existing == :code.which(Axon.Watcher.Schemas.ExtractionResult)
    assert :non_existing == :code.which(Axon.Watcher.Progress)
    assert :non_existing == :code.which(Axon.Watcher.ProjectMetrics)
  end

  test "MIL-AXO-028 cat A — SQLite Ecto Repo config purged" do
    assert Application.get_env(:axon_dashboard, :ecto_repos) == nil
    assert Application.get_env(:axon_dashboard, Axon.Watcher.Repo) == nil
  end

  # REQ-AXO-901802 (MIL-AXO-028 cat B) — System.get_env centralization.
  # All AXON_* env vars are read in config/runtime.exs only (or test.exs
  # for the test env). Consumers in lib/ read via Application.get_env.
  test "MIL-AXO-028 cat B — System.get_env eliminated from lib/ modules" do
    # SqlGateway: url + allow_cross_instance_fallback come from Application.env
    config = Application.get_env(:axon_dashboard, Axon.Watcher.SqlGateway, [])
    assert Keyword.has_key?(config, :url)
    assert Keyword.get(config, :allow_cross_instance_fallback) == false

    # McpClient: endpoint comes from Application.env
    mcp_config = Application.get_env(:axon_dashboard, Axon.Watcher.McpClient, [])
    assert Keyword.has_key?(mcp_config, :endpoint)

    # IndexerHeartbeat: path comes from Application.env
    hb_config = Application.get_env(:axon_dashboard, Axon.Watcher.IndexerHeartbeat, [])
    assert Keyword.has_key?(hb_config, :path)

    # BridgeClient: telemetry_socket_path comes from Application.env
    # (legacy global key or per-module — at least one must be set)
    legacy_key = Application.get_env(:axon_dashboard, :telemetry_socket_path)
    bridge_config = Application.get_env(:axon_dashboard, AxonDashboard.BridgeClient, [])
    assert legacy_key != nil or Keyword.has_key?(bridge_config, :telemetry_socket_path)

    # instance_kind exposed for LiveView badges
    assert Application.get_env(:axon_dashboard, :instance_kind) != nil
  end
end
