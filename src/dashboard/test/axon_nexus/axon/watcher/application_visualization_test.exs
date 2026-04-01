# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.ApplicationVisualizationTest do
  use ExUnit.Case, async: false

  test "watcher application child specs exclude canonical ingestion authority" do
    child_ids =
      Axon.Watcher.Application.visualization_children()
      |> Enum.map(&Supervisor.child_spec(&1, []).id)

    refute Axon.Watcher.Staging in child_ids
    refute Oban in child_ids
    refute Axon.Watcher.Server in child_ids
    refute Axon.Watcher.TrafficGuardian in child_ids
    refute Axon.Watcher.PoolFacade in child_ids
  end
end
