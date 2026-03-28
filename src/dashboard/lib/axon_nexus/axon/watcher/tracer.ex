defmodule Axon.Watcher.Tracer do
  @moduledoc """
  System Observability Tracer (Mission Critical Pattern).
  Tracks microsecond latencies across the ingestion pipeline:
  T0: Inotify event received
  T1: Enqueued to Oban/Rust
  T2: Picked up by Rust worker
  T3: Processed by Rust (ONNX/Parsing)
  T4: Committed to KuzuDB and returned to Elixir

  Maintains ETS tables for P50, P99 calculation and dashboard exposition.
  """
  use GenServer
  require Logger

  @table :axon_system_tracer

  def start_link(_) do
    GenServer.start_link(__MODULE__, :ok, name: __MODULE__)
  end

  def init(:ok) do
    # Lock-free concurrency with public access for fast writes
    :ets.new(@table, [
      :set,
      :public,
      :named_table,
      read_concurrency: true,
      write_concurrency: true
    ])

    :ets.insert(@table, {:metrics,
     %{
       count: 0,
       # T0 -> T1
       t1_latencies: [],
       # T1 -> T2
       t2_latencies: [],
       # T2 -> T3
       t3_latencies: [],
       # T3 -> T4
       t4_latencies: [],
       # T0 -> T4
       total_latencies: []
     }})

    # Dump report every 10 seconds if active
    :timer.send_interval(10_000, self(), :dump_report)

    {:ok, %{}}
  end

  def record_trace(trace_id, path, t0, t1, t2, t3, t4) do
    GenServer.cast(__MODULE__, {:record, trace_id, path, t0, t1, t2, t3, t4})
  end

  def handle_cast({:record, _trace_id, path, t0, t1, t2, t3, t4}, state) do
    # Convert microseconds to milliseconds for easier reading
    lat_t1 = max(0, (t1 - t0) / 1000.0)
    lat_t2 = max(0, (t2 - t1) / 1000.0)
    lat_t3 = max(0, (t3 - t2) / 1000.0)
    lat_t4 = max(0, (t4 - t3) / 1000.0)

    # Emit telemetry event for TrafficGuardian and other observers
    :telemetry.execute([:axon, :watcher, :file_indexed], %{t4: lat_t4}, %{path: path})

    t5 = :os.system_time(:microsecond)
    total_lat = max(0, (t5 - t0) / 1000.0)

    case :ets.lookup(@table, :metrics) do
      [{:metrics, stats}] ->
        new_stats = %{
          count: stats.count + 1,
          t1_latencies: keep_last(stats.t1_latencies, lat_t1, 1000),
          t2_latencies: keep_last(stats.t2_latencies, lat_t2, 1000),
          t3_latencies: keep_last(stats.t3_latencies, lat_t3, 1000),
          t4_latencies: keep_last(stats.t4_latencies, lat_t4, 1000),
          total_latencies: keep_last(stats.total_latencies, total_lat, 1000)
        }

        :ets.insert(@table, {:metrics, new_stats})

      _ ->
        :ok
    end

    {:noreply, state}
  end

  def handle_info(:dump_report, state) do
    case :ets.lookup(@table, :metrics) do
      [{:metrics, stats}] when stats.count > 0 ->
        p99_total = calculate_p99(stats.total_latencies)
        p99_queue = calculate_p99(stats.t2_latencies)
        p99_ai = calculate_p99(stats.t3_latencies)
        p99_db = calculate_p99(stats.t4_latencies)

        Logger.info("""
        [TRACER] ☢️ SYSTEM OBSERVABILITY (Last 1000 files)
        - Throughput: #{stats.count} files ingested.
        - P99 E2E Latency: #{Float.round(p99_total, 2)} ms
        - Breakdown (P99):
          * Ingress & Oban Queue: #{Float.round(calculate_p99(stats.t1_latencies), 2)} ms
          * Rust Queue Wait:      #{Float.round(p99_queue, 2)} ms
          * CPU Parse & AI Embed: #{Float.round(p99_ai, 2)} ms
          * KuzuDB Actor Write:   #{Float.round(p99_db, 2)} ms
        """)

        # Reset counters to only track active window
        :ets.insert(@table, {:metrics, %{stats | count: 0}})

      _ ->
        :ok
    end

    {:noreply, state}
  end

  defp keep_last(list, val, max_len) do
    Enum.take([val | list], max_len)
  end

  defp calculate_p99([]) do
    0.0
  end

  defp calculate_p99(list) do
    sorted = Enum.sort(list)
    index = ceil(length(sorted) * 0.99) - 1
    index = max(0, min(index, length(sorted) - 1))
    Enum.at(sorted, index)
  end
end
