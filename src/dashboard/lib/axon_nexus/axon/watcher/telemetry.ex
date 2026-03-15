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
    :ets.insert(:axon_telemetry, {:active_workers, %{}})
    :ets.insert(:axon_telemetry, {:last_files, []})
    :ets.insert(:axon_telemetry, {:directories, %{}}) # {dir_name => %{total, completed, failed, last_update}}
    {:ok, %{}}
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
    new_workers = Map.put(workers, worker_id, %{file: Path.basename(file_path), start: System.monotonic_time()})
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
        if status == :ok do
          %{dirs[dir] | completed: dirs[dir].completed + 1, last_update: now}
        else
          %{dirs[dir] | failed: dirs[dir].failed + 1, last_update: now}
        end
      :ets.insert(:axon_telemetry, {:directories, Map.put(dirs, dir, updated_dir)})
    end
  end

  def get_stats do
    %{
      active_workers: get_val(:active_workers),
      last_files: get_val(:last_files),
      directories: get_val(:directories)
    }
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
