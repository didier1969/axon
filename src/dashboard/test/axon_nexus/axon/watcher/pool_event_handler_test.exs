defmodule Axon.Watcher.PoolEventHandlerTest do
  use ExUnit.Case, async: false

  alias Axon.Watcher.PoolEventHandler

  test "process_pending is visualization-only and emits an ignored checkpoint" do
    batch = [%{"path" => "/tmp/example.ex", "trace_id" => "trace-1", "priority" => 900}]
    parent = self()
    handler_id = "pending-ignored-#{System.unique_integer([:positive])}"

    :telemetry.attach(
      handler_id,
      [:axon, :watcher, :pending_batch_ignored],
      fn _event, measurements, metadata, pid ->
        send(pid, {:pending_batch_ignored, measurements, metadata})
      end,
      parent
    )

    on_exit(fn -> :telemetry.detach(handler_id) end)

    assert :ok = PoolEventHandler.process_pending(batch)
    assert_receive {:pending_batch_ignored, %{count: 1}, %{paths: ["/tmp/example.ex"]}}, 1000
  end
end
