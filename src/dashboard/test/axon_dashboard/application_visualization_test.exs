defmodule AxonDashboard.ApplicationVisualizationTest do
  use ExUnit.Case, async: false

  test "dashboard supervisor does not boot canonical ingestion authority" do
    child_ids =
      AxonDashboard.Supervisor
      |> Supervisor.which_children()
      |> Enum.map(fn {id, _pid, _type, _modules} -> id end)

    refute Oban in child_ids
    refute Axon.Watcher.Server in child_ids
  end
end
