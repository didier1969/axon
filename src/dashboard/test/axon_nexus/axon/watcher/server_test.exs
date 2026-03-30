defmodule Axon.Watcher.ServerTest do
  use ExUnit.Case, async: false

  alias Axon.Watcher.Server

  setup do
    parent = self()
    manual_handler = "manual-scan-#{System.unique_integer([:positive])}"
    forwarded_handler = "scan-forwarded-#{System.unique_integer([:positive])}"

    :telemetry.attach(
      manual_handler,
      [:axon, :watcher, :manual_scan_triggered],
      fn _event, measurements, metadata, pid ->
        send(pid, {:manual_scan_triggered, measurements, metadata})
      end,
      parent
    )

    :telemetry.attach(
      forwarded_handler,
      [:axon, :watcher, :scan_forwarded],
      fn _event, measurements, metadata, pid ->
        send(pid, {:scan_forwarded, measurements, metadata})
      end,
      parent
    )

    on_exit(fn ->
      :telemetry.detach(manual_handler)
      :telemetry.detach(forwarded_handler)
    end)

    :ok
  end

  test "trigger_scan emits operator and forwarding telemetry" do
    Server.trigger_scan()

    assert_receive {:manual_scan_triggered, %{count: 1}, %{repo_slug: _repo, watch_dir: _dir}}, 1000
    assert_receive {:scan_forwarded, %{count: 1}, %{connected: connected}}, 1000
    assert is_boolean(connected)
  end
end
