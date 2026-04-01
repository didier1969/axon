# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboard.TelemetryHandlerTest do
  use ExUnit.Case, async: false

  setup do
    Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "telemetry_events")
    :ok
  end

  test "relays backpressure telemetry to the dashboard bus" do
    :telemetry.execute(
      [:axon, :backpressure, :pressure_computed],
      %{pressure: 0.42},
      %{cpu: 10.0, ram: 20.0, io: 1.0}
    )

    assert_receive {:telemetry_event, [:axon, :backpressure, :pressure_computed], %{pressure: 0.42}, %{cpu: 10.0, ram: 20.0, io: 1.0}}, 1000
  end

  test "does not relay legacy watcher enqueue telemetry" do
    :telemetry.execute(
      [:axon, :watcher, :batch_enqueued],
      %{count: 3},
      %{queue: :indexing_default}
    )

    refute_receive {:telemetry_event, [:axon, :watcher, :batch_enqueued], _, _}, 200
  end
end
