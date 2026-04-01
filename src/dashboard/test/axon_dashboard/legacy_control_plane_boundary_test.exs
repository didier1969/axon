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
end
