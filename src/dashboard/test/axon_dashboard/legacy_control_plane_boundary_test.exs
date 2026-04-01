# Copyright (c) Didier Stadelmann. All rights reserved.
defmodule AxonDashboard.LegacyControlPlaneBoundaryTest do
  use ExUnit.Case, async: true

  test "dashboard no longer configures Oban as a canonical ingestion queue" do
    assert Application.get_env(:axon_dashboard, Oban) == nil
  end

  test "pool facade no longer exposes legacy batch admission commands" do
    refute function_exported?(Axon.Watcher.PoolFacade, :parse_batch, 1)
    refute function_exported?(Axon.Watcher.PoolFacade, :pull_pending, 1)
  end

  test "pool protocol no longer exposes legacy batch acknowledgements" do
    refute function_exported?(Axon.Watcher.PoolProtocol, :ack_targets, 2)
  end

  test "dead read-side legacy modules are no longer compiled into the dashboard" do
    assert :non_existing == :code.which(AxonDashboardWeb.StatusLive)
    assert :non_existing == :code.which(Axon.Watcher.StatsCache)
    assert :non_existing == :code.which(Axon.Watcher.PoolEventHandler)
    assert :non_existing == :code.which(Axon.Watcher.Auditor)
    assert :non_existing == :code.which(Axon.Watcher.Tracking)
    assert :non_existing == :code.which(Axon.Watcher.IndexedProject)
    assert :non_existing == :code.which(Axon.Watcher.IndexedFile)
  end
end
