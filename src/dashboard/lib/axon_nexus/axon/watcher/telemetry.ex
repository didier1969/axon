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
    :ets.insert(:axon_telemetry, {:budget_bytes, Map.get(payload, "budget_bytes", 0)})
    :ets.insert(:axon_telemetry, {:reserved_bytes, Map.get(payload, "reserved_bytes", 0)})
    :ets.insert(:axon_telemetry, {:exhaustion_ratio, Map.get(payload, "exhaustion_ratio", 0.0)})
    :ets.insert(:axon_telemetry, {:queue_depth, Map.get(payload, "queue_depth", 0)})
    :ets.insert(:axon_telemetry, {:claim_mode, Map.get(payload, "claim_mode", "unknown")})

    :ets.insert(
      :axon_telemetry,
      {:oversized_refusals_total, Map.get(payload, "oversized_refusals_total", 0)}
    )

    :ets.insert(
      :axon_telemetry,
      {:degraded_mode_entries_total, Map.get(payload, "degraded_mode_entries_total", 0)}
    )

    :ets.insert(
      :axon_telemetry,
      {:service_pressure, Map.get(payload, "service_pressure", "healthy")}
    )

    :ets.insert(:axon_telemetry, {:cpu_load, Map.get(payload, "cpu_load", 0.0)})
    :ets.insert(:axon_telemetry, {:ram_load, Map.get(payload, "ram_load", 0.0)})
    :ets.insert(:axon_telemetry, {:io_wait, Map.get(payload, "io_wait", 0.0)})
    :ets.insert(:axon_telemetry, {:host_state, Map.get(payload, "host_state", "healthy")})
    :ets.insert(:axon_telemetry, {:rss_bytes, Map.get(payload, "rss_bytes", 0)})
    :ets.insert(:axon_telemetry, {:rss_anon_bytes, Map.get(payload, "rss_anon_bytes", 0)})
    :ets.insert(:axon_telemetry, {:rss_file_bytes, Map.get(payload, "rss_file_bytes", 0)})
    :ets.insert(:axon_telemetry, {:rss_shmem_bytes, Map.get(payload, "rss_shmem_bytes", 0)})
    :ets.insert(:axon_telemetry, {:db_file_bytes, Map.get(payload, "db_file_bytes", 0)})
    :ets.insert(:axon_telemetry, {:db_wal_bytes, Map.get(payload, "db_wal_bytes", 0)})
    :ets.insert(:axon_telemetry, {:db_total_bytes, Map.get(payload, "db_total_bytes", 0)})

    :ets.insert(
      :axon_telemetry,
      {:duckdb_memory_bytes, Map.get(payload, "duckdb_memory_bytes", 0)}
    )

    :ets.insert(
      :axon_telemetry,
      {:duckdb_temporary_bytes, Map.get(payload, "duckdb_temporary_bytes", 0)}
    )

    :ets.insert(
      :axon_telemetry,
      {:host_guidance_slots, Map.get(payload, "host_guidance_slots", 0)}
    )
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
    %{
      active_workers: get_val(:active_workers),
      last_files: get_val(:last_files),
      directories: get_val(:directories),
      target_pressure: get_val(:target_pressure),
      t4_ema: get_val(:t4_ema),
      flux_reel: get_val(:flux_reel),
      total_ingested: get_val(:total_ingested),
      budget_bytes: get_val(:budget_bytes),
      reserved_bytes: get_val(:reserved_bytes),
      exhaustion_ratio: get_val(:exhaustion_ratio),
      queue_depth: get_val(:queue_depth),
      claim_mode: get_val(:claim_mode),
      service_pressure: get_val(:service_pressure),
      oversized_refusals_total: get_val(:oversized_refusals_total),
      degraded_mode_entries_total: get_val(:degraded_mode_entries_total),
      cpu_load: get_val(:cpu_load),
      ram_load: get_val(:ram_load),
      io_wait: get_val(:io_wait),
      host_state: get_val(:host_state),
      host_guidance_slots: get_val(:host_guidance_slots),
      rss_bytes: get_val(:rss_bytes),
      rss_anon_bytes: get_val(:rss_anon_bytes),
      rss_file_bytes: get_val(:rss_file_bytes),
      rss_shmem_bytes: get_val(:rss_shmem_bytes),
      db_file_bytes: get_val(:db_file_bytes),
      db_wal_bytes: get_val(:db_wal_bytes),
      db_total_bytes: get_val(:db_total_bytes),
      duckdb_memory_bytes: get_val(:duckdb_memory_bytes),
      duckdb_temporary_bytes: get_val(:duckdb_temporary_bytes)
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
    :ets.insert(:axon_telemetry, {:budget_bytes, 0})
    :ets.insert(:axon_telemetry, {:reserved_bytes, 0})
    :ets.insert(:axon_telemetry, {:exhaustion_ratio, 0.0})
    :ets.insert(:axon_telemetry, {:queue_depth, 0})
    :ets.insert(:axon_telemetry, {:claim_mode, "unknown"})
    :ets.insert(:axon_telemetry, {:service_pressure, "healthy"})
    :ets.insert(:axon_telemetry, {:oversized_refusals_total, 0})
    :ets.insert(:axon_telemetry, {:degraded_mode_entries_total, 0})
    :ets.insert(:axon_telemetry, {:cpu_load, 0.0})
    :ets.insert(:axon_telemetry, {:ram_load, 0.0})
    :ets.insert(:axon_telemetry, {:io_wait, 0.0})
    :ets.insert(:axon_telemetry, {:host_state, "healthy"})
    :ets.insert(:axon_telemetry, {:host_guidance_slots, 0})
    :ets.insert(:axon_telemetry, {:rss_bytes, 0})
    :ets.insert(:axon_telemetry, {:rss_anon_bytes, 0})
    :ets.insert(:axon_telemetry, {:rss_file_bytes, 0})
    :ets.insert(:axon_telemetry, {:rss_shmem_bytes, 0})
    :ets.insert(:axon_telemetry, {:db_file_bytes, 0})
    :ets.insert(:axon_telemetry, {:db_wal_bytes, 0})
    :ets.insert(:axon_telemetry, {:db_total_bytes, 0})
    :ets.insert(:axon_telemetry, {:duckdb_memory_bytes, 0})
    :ets.insert(:axon_telemetry, {:duckdb_temporary_bytes, 0})
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
