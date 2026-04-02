# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.Telemetry do
  @moduledoc """
  In-memory store for live cockpit metrics.
  Uses ETS for sub-millisecond performance.
  Tracks progress per directory.
  """
  use GenServer

  def start_link(_) do
    GenServer.start_link(__MODULE__, :ok, name: __MODULE__)
  end

  def init(:ok) do
    :ets.new(:axon_telemetry, [:set, :public, :named_table])
    reset()
    {:ok, %{}}
  end

  def reset! do
    reset()
  end

  def update_backpressure(pressure, ema) do
    :ets.insert(:axon_telemetry, {:target_pressure, pressure})
    :ets.insert(:axon_telemetry, {:t4_ema, ema})
  end

  def update_flux(flux) do
    :ets.insert(:axon_telemetry, {:flux_reel, flux})
  end

  def update_runtime_telemetry(payload) when is_map(payload) do
    runtime_snapshot = %{
      budget_bytes: Map.get(payload, "budget_bytes", 0),
      reserved_bytes: Map.get(payload, "reserved_bytes", 0),
      exhaustion_ratio: Map.get(payload, "exhaustion_ratio", 0.0),
      reserved_task_count: Map.get(payload, "reserved_task_count", 0),
      anonymous_trace_reserved_tasks: Map.get(payload, "anonymous_trace_reserved_tasks", 0),
      anonymous_trace_admissions_total: Map.get(payload, "anonymous_trace_admissions_total", 0),
      reservation_release_misses_total:
        Map.get(payload, "reservation_release_misses_total", 0),
      queue_depth: Map.get(payload, "queue_depth", 0),
      claim_mode: Map.get(payload, "claim_mode", "unknown"),
      oversized_refusals_total: Map.get(payload, "oversized_refusals_total", 0),
      degraded_mode_entries_total: Map.get(payload, "degraded_mode_entries_total", 0),
      service_pressure: Map.get(payload, "service_pressure", "healthy"),
      cpu_load: Map.get(payload, "cpu_load", 0.0),
      ram_load: Map.get(payload, "ram_load", 0.0),
      io_wait: Map.get(payload, "io_wait", 0.0),
      host_state: Map.get(payload, "host_state", "healthy"),
      host_guidance_slots: Map.get(payload, "host_guidance_slots", 0),
      rss_bytes: Map.get(payload, "rss_bytes", 0),
      rss_anon_bytes: Map.get(payload, "rss_anon_bytes", 0),
      rss_file_bytes: Map.get(payload, "rss_file_bytes", 0),
      rss_shmem_bytes: Map.get(payload, "rss_shmem_bytes", 0),
      db_file_bytes: Map.get(payload, "db_file_bytes", 0),
      db_wal_bytes: Map.get(payload, "db_wal_bytes", 0),
      db_total_bytes: Map.get(payload, "db_total_bytes", 0),
      duckdb_memory_bytes: Map.get(payload, "duckdb_memory_bytes", 0),
      duckdb_temporary_bytes: Map.get(payload, "duckdb_temporary_bytes", 0),
      ingress_enabled: Map.get(payload, "ingress_enabled", false),
      ingress_buffered_entries: Map.get(payload, "ingress_buffered_entries", 0),
      ingress_subtree_hints: Map.get(payload, "ingress_subtree_hints", 0),
      ingress_collapsed_total: Map.get(payload, "ingress_collapsed_total", 0),
      ingress_flush_count: Map.get(payload, "ingress_flush_count", 0),
      ingress_last_flush_duration_ms: Map.get(payload, "ingress_last_flush_duration_ms", 0),
      ingress_last_promoted_count: Map.get(payload, "ingress_last_promoted_count", 0)
    }

    :ets.insert(:axon_telemetry, {:runtime_snapshot, runtime_snapshot})
  end

  def init_directories(files) do
    # Group files by top-level directory
    dir_map =
      Enum.reduce(files, %{}, fn path, acc ->
        dir = get_top_dir(path)

        Map.update(acc, dir, %{total: 1, completed: 0, failed: 0, last_update: nil}, fn stats ->
          %{stats | total: stats.total + 1}
        end)
      end)

    :ets.insert(:axon_telemetry, {:directories, dir_map})
  end

  def report_start(worker_id, file_path) do
    workers = get_val(:active_workers)

    new_workers =
      Map.put(workers, worker_id, %{
        file: Path.basename(file_path),
        start: System.monotonic_time()
      })

    :ets.insert(:axon_telemetry, {:active_workers, new_workers})
  end

  def report_finish(worker_id, file_path, status) do
    workers = get_val(:active_workers)
    :ets.insert(:axon_telemetry, {:active_workers, Map.delete(workers, worker_id)})

    now = DateTime.utc_now()

    # Update Last Files
    last_files = get_val(:last_files)
    new_last = [%{path: file_path, status: status, time: now} | Enum.take(last_files, 14)]
    :ets.insert(:axon_telemetry, {:last_files, new_last})

    # Update Directory Stats
    dir = get_top_dir(file_path)
    dirs = get_val(:directories)

    if Map.has_key?(dirs, dir) do
      updated_dir =
        if status in [:ok, :degraded] do
          %{dirs[dir] | completed: dirs[dir].completed + 1, last_update: now}
        else
          %{dirs[dir] | failed: dirs[dir].failed + 1, last_update: now}
        end

      :ets.insert(:axon_telemetry, {:directories, Map.put(dirs, dir, updated_dir)})
    end

    :ets.insert(:axon_telemetry, {:total_ingested, get_val(:total_ingested) + 1})
  end

  def get_stats do
    runtime = get_val(:runtime_snapshot)

    %{
      active_workers: get_val(:active_workers),
      last_files: get_val(:last_files),
      directories: get_val(:directories),
      target_pressure: get_val(:target_pressure),
      t4_ema: get_val(:t4_ema),
      flux_reel: get_val(:flux_reel),
      total_ingested: get_val(:total_ingested),
      budget_bytes: Map.get(runtime, :budget_bytes, 0),
      reserved_bytes: Map.get(runtime, :reserved_bytes, 0),
      exhaustion_ratio: Map.get(runtime, :exhaustion_ratio, 0.0),
      reserved_task_count: Map.get(runtime, :reserved_task_count, 0),
      anonymous_trace_reserved_tasks: Map.get(runtime, :anonymous_trace_reserved_tasks, 0),
      anonymous_trace_admissions_total: Map.get(runtime, :anonymous_trace_admissions_total, 0),
      reservation_release_misses_total:
        Map.get(runtime, :reservation_release_misses_total, 0),
      queue_depth: Map.get(runtime, :queue_depth, 0),
      claim_mode: Map.get(runtime, :claim_mode, "unknown"),
      service_pressure: Map.get(runtime, :service_pressure, "healthy"),
      oversized_refusals_total: Map.get(runtime, :oversized_refusals_total, 0),
      degraded_mode_entries_total: Map.get(runtime, :degraded_mode_entries_total, 0),
      cpu_load: Map.get(runtime, :cpu_load, 0.0),
      ram_load: Map.get(runtime, :ram_load, 0.0),
      io_wait: Map.get(runtime, :io_wait, 0.0),
      host_state: Map.get(runtime, :host_state, "healthy"),
      host_guidance_slots: Map.get(runtime, :host_guidance_slots, 0),
      rss_bytes: Map.get(runtime, :rss_bytes, 0),
      rss_anon_bytes: Map.get(runtime, :rss_anon_bytes, 0),
      rss_file_bytes: Map.get(runtime, :rss_file_bytes, 0),
      rss_shmem_bytes: Map.get(runtime, :rss_shmem_bytes, 0),
      db_file_bytes: Map.get(runtime, :db_file_bytes, 0),
      db_wal_bytes: Map.get(runtime, :db_wal_bytes, 0),
      db_total_bytes: Map.get(runtime, :db_total_bytes, 0),
      duckdb_memory_bytes: Map.get(runtime, :duckdb_memory_bytes, 0),
      duckdb_temporary_bytes: Map.get(runtime, :duckdb_temporary_bytes, 0),
      ingress_enabled: Map.get(runtime, :ingress_enabled, false),
      ingress_buffered_entries: Map.get(runtime, :ingress_buffered_entries, 0),
      ingress_subtree_hints: Map.get(runtime, :ingress_subtree_hints, 0),
      ingress_collapsed_total: Map.get(runtime, :ingress_collapsed_total, 0),
      ingress_flush_count: Map.get(runtime, :ingress_flush_count, 0),
      ingress_last_flush_duration_ms: Map.get(runtime, :ingress_last_flush_duration_ms, 0),
      ingress_last_promoted_count: Map.get(runtime, :ingress_last_promoted_count, 0)
    }
  end

  defp reset do
    :ets.insert(:axon_telemetry, {:active_workers, %{}})
    :ets.insert(:axon_telemetry, {:last_files, []})
    # {dir_name => %{total, completed, failed, last_update}}
    :ets.insert(:axon_telemetry, {:directories, %{}})
    :ets.insert(:axon_telemetry, {:target_pressure, 100})
    :ets.insert(:axon_telemetry, {:t4_ema, 0.0})
    :ets.insert(:axon_telemetry, {:flux_reel, 0.0})
    :ets.insert(:axon_telemetry, {:total_ingested, 0})
    :ets.insert(
      :axon_telemetry,
      {:runtime_snapshot,
       %{
         budget_bytes: 0,
         reserved_bytes: 0,
         exhaustion_ratio: 0.0,
         reserved_task_count: 0,
         anonymous_trace_reserved_tasks: 0,
         anonymous_trace_admissions_total: 0,
         reservation_release_misses_total: 0,
         queue_depth: 0,
         claim_mode: "unknown",
         service_pressure: "healthy",
         oversized_refusals_total: 0,
         degraded_mode_entries_total: 0,
         cpu_load: 0.0,
         ram_load: 0.0,
         io_wait: 0.0,
         host_state: "healthy",
         host_guidance_slots: 0,
         rss_bytes: 0,
         rss_anon_bytes: 0,
         rss_file_bytes: 0,
         rss_shmem_bytes: 0,
         db_file_bytes: 0,
         db_wal_bytes: 0,
         db_total_bytes: 0,
         duckdb_memory_bytes: 0,
         duckdb_temporary_bytes: 0,
         ingress_enabled: false,
         ingress_buffered_entries: 0,
         ingress_subtree_hints: 0,
         ingress_collapsed_total: 0,
         ingress_flush_count: 0,
         ingress_last_flush_duration_ms: 0,
         ingress_last_promoted_count: 0
       }}
    )
  end

  defp get_val(key) do
    case :ets.lookup(:axon_telemetry, key) do
      [{^key, val}] -> val
      _ -> nil
    end
  end

  defp get_top_dir(path) do
    # Extract the first directory name relative to the current working directory
    relative_path = Path.relative_to(path, File.cwd!())
    parts = Path.split(relative_path)

    case parts do
      [dir | _] when dir != "." -> dir
      _ -> "root"
    end
  end
end
