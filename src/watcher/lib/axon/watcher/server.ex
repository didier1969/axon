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

    initial_state = %{
      repo_slug: repo_slug, 
      watcher_pid: nil, 
      watch_dir: watch_dir, 
      pending_files: MapSet.new(), 
      timer: nil, 
      monitoring_active: true,
      pending_batches: %{100 => [], 80 => [], 50 => [], 10 => []}
    }

    case FileSystem.start_link(dirs: [watch_dir]) do
      {:ok, watcher_pid} ->
        FileSystem.subscribe(watcher_pid)
        {:ok, %{initial_state | watcher_pid: watcher_pid}, {:continue, :auto_trigger_scan}}
      _ ->
        {:ok, initial_state, {:continue, :auto_trigger_scan}}
    end
  end

  @impl true
  def handle_continue(:auto_trigger_scan, state) do
    Logger.info("[Pod A] AUTO-START: Triggering initial scan...")
    send(self(), :initial_scan)
    {:noreply, state}
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
    Logger.info("[Pod A] Triggering Reactive Streaming Scan on: #{state.watch_dir}")
    Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "indexing", total: 0, progress: 0})
    # Déclenche le scan asynchrone qui enverra des messages {:ok, path} ou {:ok, "done"}
    Axon.Scanner.start_streaming(state.watch_dir, self())
    {:noreply, state}
  end

  @impl true
  def handle_info({:ok, "done"}, state) do
    Logger.info("[Pod A] Reactive Scan Completed.")
    flush_all_batches(state.pending_batches)
    Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", progress: 100})
    {:noreply, %{state | pending_batches: %{100 => [], 80 => [], 50 => [], 10 => []}}}
  end

  @impl true
  def handle_info({:ok, path}, state) do
    str_path = to_string(path)
    if should_process?(str_path) do
      priority = calculate_priority(str_path)
      
      case File.stat(str_path) do
        {:ok, %{mtime: mtime}} ->
          last_mtime = Axon.Watcher.Progress.get_file_mtime(state.repo_slug, str_path)
          current_mtime = :erlang.phash2(mtime)
          if current_mtime != last_mtime do
             Axon.Watcher.Progress.save_file_mtime(state.repo_slug, str_path, current_mtime)
             
             current_batch = state.pending_batches[priority]
             new_batch = [str_path | current_batch]
             
             threshold = if priority >= 80, do: 10, else: @max_batch_size
             
             if length(new_batch) >= threshold do
               queue = if priority >= 80, do: :indexing_hot, else: :indexing_default
               dispatch_batch(new_batch, queue)
               {:noreply, put_in(state.pending_batches[priority], [])}
             else
               {:noreply, put_in(state.pending_batches[priority], new_batch)}
             end
          else
            {:noreply, state}
          end
        _ -> {:noreply, state}
      end
    else
      {:noreply, state}
    end
  end

  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    str_path = to_string(path)
    if state.monitoring_active and should_process?(str_path) do
      if :deleted in events do
        {:noreply, state}
      else
        parent_dir = Path.dirname(str_path)
        
        # Obtenir les fichiers voisins (proximité architecturale)
        neighbors = 
          case File.ls(parent_dir) do
            {:ok, files} -> 
              Enum.map(files, &Path.join(parent_dir, &1))
              |> Enum.filter(&should_process?/1)
              |> Enum.filter(&(File.regular?(&1)))
            _ -> []
          end
        
        # Fusionner avec les fichiers en attente
        new_pending = Enum.reduce([str_path | neighbors], state.pending_files, &MapSet.put(&2, &1))
        
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
    if length(files_to_process) > 0 do
      files_to_process 
      |> Enum.chunk_every(@max_batch_size) 
      |> Enum.each(&dispatch_batch(&1, :indexing_hot))
    end
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

  defp dispatch_batch(paths, queue) do
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
        Axon.Watcher.IndexingWorker.new(job_args, queue: queue)
        |> Oban.insert!()
        Logger.info("[Pod A] Enqueued batch of #{length(files_payload)} files to #{queue}.")
      rescue
        e -> Logger.error("[Pod A] FAILED to enqueue batch: #{inspect(e)}")
      end
    end
  end

  defp calculate_priority(path) do
    ext = Path.extname(path) |> String.downcase()
    cond do
      ext in [".ex", ".exs", ".rs", ".py", ".go"] -> 100
      ext in [".js", ".ts", ".c", ".cpp", ".h"] -> 80
      ext in [".md", ".txt", ".json", ".yml", ".yaml", ".toml", ".conf"] -> 50
      true -> 10
    end
  end

  defp flush_all_batches(batches) do
    Enum.each(batches, fn {priority, paths} ->
      if length(paths) > 0 do
        queue = if priority >= 80, do: :indexing_hot, else: :indexing_default
        dispatch_batch(paths, queue)
      end
    end)
  end
end
