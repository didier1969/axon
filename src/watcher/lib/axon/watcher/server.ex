defmodule Axon.Watcher.Server do
  @moduledoc """
  The core orchestrator of Pod A.
  Watches the filesystem, batches events, and dispatches to the Worker Pool.
  Now with strict Binary Filtering to prevent Ecto/Oban crashes.
  """
  use GenServer
  require Logger

  @batch_timeout 500
  @max_batch_size 20

  # --- Client API ---

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def trigger_scan, do: GenServer.cast(__MODULE__, :trigger_scan)
  def pause_monitoring, do: GenServer.cast(__MODULE__, :pause_monitoring)
  def resume_monitoring, do: GenServer.cast(__MODULE__, :resume_monitoring)
  def purge_data, do: GenServer.call(__MODULE__, :purge_data)
  def get_monitoring_status, do: GenServer.call(__MODULE__, :get_monitoring_status)

  # --- Server Callbacks ---

  @impl true
  def init(opts) do
    repo_slug = System.get_env("AXON_REPO_SLUG") || (Path.expand(".") |> Path.basename())
    env_dir = System.get_env("AXON_WATCH_DIR")
    default_dir = Path.expand("../../../../../", __DIR__)
    watch_dir = Keyword.get(opts, :dir, env_dir || default_dir)
    Logger.info("Pod A (Watcher) starting supervision on: #{watch_dir}")

    # Nettoyage initial des logs pour éviter les boucles
    Axon.Watcher.Progress.update_status(repo_slug, %{status: "live", progress: 0})

    case FileSystem.start_link(dirs: [watch_dir]) do
      {:ok, watcher_pid} ->
        FileSystem.subscribe(watcher_pid)
        {:ok, %{repo_slug: repo_slug, watcher_pid: watcher_pid, watch_dir: watch_dir, pending_files: MapSet.new(), timer: nil, monitoring_active: true}}
      _ ->
        {:ok, %{repo_slug: repo_slug, watcher_pid: nil, watch_dir: watch_dir, pending_files: MapSet.new(), timer: nil, monitoring_active: true}}
    end
  end

  @impl true
  def handle_cast(:trigger_scan, state) do
    send(self(), :initial_scan)
    {:noreply, state}
  end

  @impl true
  def handle_cast(:pause_monitoring, state), do: {:noreply, %{state | monitoring_active: false}}
  @impl true
  def handle_cast(:resume_monitoring, state), do: {:noreply, %{state | monitoring_active: true}}

  @impl true
  def handle_call(:purge_data, _from, state) do
    Axon.Watcher.Progress.purge_repo(state.repo_slug)
    Axon.Watcher.Repo.query("DELETE FROM oban_jobs")
    {:reply, :ok, state}
  end

  @impl true
  def handle_call(:get_monitoring_status, _from, state), do: {:reply, state.monitoring_active, state}

  @impl true
  def handle_info(:initial_scan, state) do
    Task.start(fn ->
      Logger.info("[Pod A] DEBUG: Scanning directory: #{state.watch_dir}")
      all_files = Axon.Scanner.scan(state.watch_dir)
      Logger.info("[Pod A] DEBUG: Rust NIF returned #{length(all_files)} raw files.")
      
      filtered_files = Enum.filter(all_files, &should_process?/1)
      Logger.info("[Pod A] DEBUG: After should_process? filter: #{length(filtered_files)} files.")
      
      files = Enum.filter(filtered_files, fn path ->
          case File.stat(path) do
            {:ok, %{mtime: mtime}} ->
              last_mtime = Axon.Watcher.Progress.get_file_mtime(state.repo_slug, path)
              current_mtime = :erlang.phash2(mtime)
              if current_mtime != last_mtime do
                 Axon.Watcher.Progress.save_file_mtime(state.repo_slug, path, current_mtime)
                 true
              else
                 false
              end
            _ -> true
          end
      end)

      total = length(files)
      if total > 0 do
        Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "indexing", total: total, progress: 0})
        files |> Enum.chunk_every(@max_batch_size) |> Enum.each(&dispatch_batch/1)
        Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", progress: 100})
      else
        Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", progress: 100})
      end
    end)
    {:noreply, state}
  end

  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    str_path = to_string(path)
    if state.monitoring_active and should_process?(str_path) do
      if :deleted in events do
        {:noreply, state}
      else
        new_pending = MapSet.put(state.pending_files, str_path)
        new_timer = reset_timer(state.timer)
        {:noreply, %{state | pending_files: new_pending, timer: new_timer}}
      end
    else
      {:noreply, state}
    end
  end

  @impl true
  def handle_info(:process_batch, state) do
    files_to_process = MapSet.to_list(state.pending_files)
    if length(files_to_process) > 0, do: files_to_process |> Enum.chunk_every(@max_batch_size) |> Enum.each(&dispatch_batch/1)
    {:noreply, %{state | pending_files: MapSet.new(), timer: nil}}
  end

  defp should_process?(path) do
    # FILTRAGE STRICT DES EXTENSIONS NON-TEXTE
    ext = Path.extname(path) |> String.downcase()
    is_binary = ext in [".png", ".jpg", ".jpeg", ".gif", ".pdf", ".exe", ".so", ".beam", ".zip", ".tar", ".gz", ".db", ".sqlite", ".wal", ".pid"]
    
    not (
      is_binary or
      String.contains?(path, "/.git/") or 
      String.contains?(path, "/.axon/") or 
      String.contains?(path, "/_build/") or 
      String.contains?(path, "/deps/") or 
      String.contains?(path, "__pycache__") or 
      String.ends_with?(path, ".log")
    )
  end

  defp reset_timer(existing_timer) do
    if existing_timer, do: Process.cancel_timer(existing_timer)
    Process.send_after(self(), :process_batch, @batch_timeout)
  end

  defp dispatch_batch(paths) do
    files_payload = Enum.reduce(paths, [], fn path, acc ->
      case File.read(path) do
        {:ok, content} -> 
          if String.printable?(content) do
            [%{"path" => path, "content" => content} | acc]
          else
            acc
          end
        _ -> acc
      end
    end)

    if length(files_payload) > 0 do
      try do
        # On passe explicitement une Map à Oban
        job_args = %{"batch" => files_payload}
        Axon.Watcher.IndexingWorker.new(job_args)
        |> Oban.insert!()
        Logger.info("[Pod A] Enqueued batch of #{length(files_payload)} files.")
      rescue
        e -> Logger.error("[Pod A] FAILED to enqueue batch: #{inspect(e)}")
      end
    end
  end
end
