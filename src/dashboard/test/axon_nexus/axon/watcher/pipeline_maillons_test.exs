# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.PipelineMaillonsTest do
  use ExUnit.Case, async: false
  alias AxonDashboard.BridgeClient

  setup do
    case Process.whereis(Axon.Watcher.Telemetry) do
      nil ->
        {:ok, pid} = Axon.Watcher.Telemetry.start_link([])
        %{telemetry_pid: pid}

      pid ->
        %{telemetry_pid: pid}
    end
  end

  test "bridge client is the sole telemetry ingress for the cockpit" do
    refute function_exported?(BridgeClient, :trigger_scan, 0)
  end

  test "bridge telemetry still survives file indexed events without pool facade" do
    pid = Process.whereis(BridgeClient)
    assert pid

    line = ~s({"FileIndexed": {"path": "/tmp/test.ex", "status": "ok", "t4": 500}})
    send(pid, {:tcp, nil, line <> "\n"})

    assert Process.alive?(pid)
  end

  test "file indexed events update live telemetry without pool facade" do
    pid = Process.whereis(BridgeClient)
    assert pid

    send(
      pid,
      {:tcp, nil,
       Jason.encode!(%{
         "FileIndexed" => %{
           "path" => "/tmp/test.ex",
           "status" => "ok",
           "trace_id" => "trace-telemetry",
           "t0" => 1,
           "t1" => 2,
           "t2" => 3,
           "t3" => 4,
           "t4" => 5,
           "symbol_count" => 7,
           "relation_count" => 2
         }
       }) <> "\n"}
    )

    stats =
      wait_for(fn ->
        stats = Axon.Watcher.Telemetry.get_stats()

        case stats[:last_files] do
          [%{path: "/tmp/test.ex", status: :ok} | _] ->
            if stats[:total_ingested] >= 1, do: stats, else: nil

          _ ->
            nil
        end
      end)

    assert stats[:total_ingested] >= 1
    assert [%{path: "/tmp/test.ex", status: :ok} | _] = stats[:last_files]
  end

  test "indexed_degraded events stay visible as degraded success, not error" do
    pid = Process.whereis(BridgeClient)
    assert pid

    send(
      pid,
      {:tcp, nil,
       Jason.encode!(%{
         "FileIndexed" => %{
           "path" => "/tmp/degraded.ex",
           "status" => "indexed_degraded",
           "trace_id" => "trace-degraded",
           "t0" => 1,
           "t1" => 2,
           "t2" => 3,
           "t3" => 4,
           "t4" => 5
         }
       }) <> "\n"}
    )

    stats =
      wait_for(fn ->
        stats = Axon.Watcher.Telemetry.get_stats()

        case stats[:last_files] do
          [%{path: "/tmp/degraded.ex", status: :degraded} | _] ->
            if stats[:total_ingested] >= 1, do: stats, else: nil

          _ ->
            nil
        end
      end)

    assert [%{path: "/tmp/degraded.ex", status: :degraded} | _] = stats[:last_files]
  end

  test "runtime status updates telemetry store from canonical Rust payload" do
    pid = Process.whereis(BridgeClient)
    assert pid

    send(
      pid,
      {:tcp, nil,
       Jason.encode!(%{
         "RuntimeTelemetry" => %{
           "budget_bytes" => 2_048,
           "reserved_bytes" => 1_024,
           "exhaustion_ratio" => 0.5,
           "queue_depth" => 17,
           "claim_mode" => "guarded",
           "service_pressure" => "degraded",
           "oversized_refusals_total" => 4,
           "degraded_mode_entries_total" => 9,
           "cpu_load" => 44.5,
           "ram_load" => 55.5,
           "io_wait" => 6.5,
           "host_state" => "watch",
           "host_guidance_slots" => 4
         }
       }) <> "\n"}
    )

    assert Process.alive?(pid)

    stats =
      wait_for(fn ->
        stats = Axon.Watcher.Telemetry.get_stats()
        if stats[:budget_bytes] == 2_048, do: stats, else: nil
      end)

    assert stats[:budget_bytes] == 2_048
    assert stats[:reserved_bytes] == 1_024
    assert stats[:exhaustion_ratio] == 0.5
    assert stats[:queue_depth] == 17
    assert stats[:claim_mode] == "guarded"
    assert stats[:service_pressure] == "degraded"
    assert stats[:oversized_refusals_total] == 4
    assert stats[:degraded_mode_entries_total] == 9
    assert stats[:cpu_load] == 44.5
    assert stats[:ram_load] == 55.5
    assert stats[:io_wait] == 6.5
    assert stats[:host_state] == "watch"
    assert stats[:host_guidance_slots] == 4
  end

  defp wait_for(fun, attempts \\ 50)

  defp wait_for(fun, attempts) when attempts > 0 do
    case fun.() do
      nil ->
        Process.sleep(10)
        wait_for(fun, attempts - 1)

      value ->
        value
    end
  end

  defp wait_for(_fun, 0), do: flunk("timed out waiting for runtime telemetry update")
end
