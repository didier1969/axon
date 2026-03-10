defmodule Axon.Watcher.Server do
  @moduledoc """
  The core orchestrator of Pod A.
  Watches the filesystem, batches events, and dispatches to the Worker Pool.
  """
  use GenServer
  require Logger

  alias Axon.Watcher.PoolFacade

  @batch_timeout 500 # 500ms de debounce
  @max_batch_size 50 # On envoie max 50 fichiers par worker

  # --- Client API ---

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  # --- Server Callbacks ---

  @impl true
  def init(opts) do
    repo_slug = System.get_env("AXON_REPO_SLUG") || "unknown"
    env_dir = System.get_env("AXON_WATCH_DIR")
    default_dir = Path.expand("../../../../../", __DIR__)
    watch_dir = Keyword.get(opts, :dir, env_dir || default_dir)
    Logger.info("Pod A (Watcher) starting supervision on: #{watch_dir}")

    # On signale le démarrage immédiatement via HydraDB
    Axon.Watcher.Progress.update_status(repo_slug, %{status: "live", progress: 0})

    case FileSystem.start_link(dirs: [watch_dir]) do
      {:ok, watcher_pid} ->
        FileSystem.subscribe(watcher_pid)
        state = %{
          repo_slug: repo_slug,
          watcher_pid: watcher_pid, 
          watch_dir: watch_dir,
          pending_files: MapSet.new(),
          timer: nil
        }
        send(self(), :initial_scan)
        {:ok, state}
      
      _error ->
        Logger.warning("FileSystem backend not available. Falling back to manual initial scan.")
        state = %{
          repo_slug: repo_slug,
          watcher_pid: nil, 
          watch_dir: watch_dir,
          pending_files: MapSet.new(),
          timer: nil
        }
        send(self(), :initial_scan)
        {:ok, state}
    end
  end

  @impl true
  def handle_info(:initial_scan, state) do
    Task.start(fn ->
      Logger.info("[Pod A] Starting high-performance Rust scan of #{state.watch_dir}")
      
      # Appel au NIF Rust ultra-rapide
      all_files = Axon.Scanner.scan(state.watch_dir)
              |> Enum.filter(&should_process?/1)
      
      # Filtrage différentiel (mtime)
      files = Enum.filter(all_files, fn path ->
          case File.stat(path) do
            {:ok, %{mtime: mtime}} ->
              last_mtime = Axon.Watcher.Progress.get_file_mtime(state.repo_slug, path)
              # On convertit le mtime Erlang en entier pour la comparaison
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
      Logger.info("[Pod A] Found #{total} files to index (changed) for #{state.repo_slug} (Total: #{length(all_files)})")
      
      if total > 0 do
        Axon.Watcher.Progress.update_status(state.repo_slug, %{
          status: "indexing", 
          total: total, 
          progress: 0, 
          synced: 0,
          last_scan_at: DateTime.utc_now() |> DateTime.to_iso8601()
        })
        
        files
        |> Enum.chunk_every(@max_batch_size)
        |> Enum.with_index()
        |> Enum.each(fn {batch, index} ->
          dispatch_batch(batch)
          
          synced = min((index + 1) * @max_batch_size, total)
          progress = round((synced / total) * 100)
          
          if rem(index, 5) == 0 or synced == total do
            Axon.Watcher.Progress.update_status(state.repo_slug, %{
              status: "indexing", 
              total: total, 
              synced: synced, 
              progress: progress,
              last_file_import_at: DateTime.utc_now() |> DateTime.to_iso8601()
            })
          end
        end)
        
        Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", total: total, synced: total, progress: 100})
        Logger.info("[Pod A] Completed initial indexing for #{state.repo_slug}")
        
        # Mode Rotation : Si AXON_SCAN_ONLY est vrai, on s'arrête proprement
        if System.get_env("AXON_SCAN_ONLY") == "true" do
           Logger.info("[Pod A] SCAN_ONLY mode active. Halting node.")
           System.halt(0)
        end
      else
        Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", total: 0, synced: 0, progress: 100})
        if System.get_env("AXON_SCAN_ONLY") == "true", do: System.halt(0)
      end
    end)
    
    {:noreply, state}
  end

  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    str_path = to_string(path)
    
    if should_process?(str_path) do
      if :deleted in events do
        Logger.info("[Pod A] Pruning requested for: #{str_path}")
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
    
    if length(files_to_process) > 0 do
      Logger.info("[Pod A] Timer expired. Batching #{length(files_to_process)} files to Pool.")
      files_to_process
      |> Enum.chunk_every(@max_batch_size)
      |> Enum.each(&dispatch_batch/1)
    end

    {:noreply, %{state | pending_files: MapSet.new(), timer: nil}}
  end

  # --- Private Logic ---

  defp should_process?(path) do
    not (
      String.contains?(path, "/.git/") or
      String.contains?(path, "/.axon/") or
      String.contains?(path, "/_build/") or
      String.contains?(path, "/deps/") or
      String.contains?(path, "__pycache__")
    )
  end

  defp reset_timer(existing_timer) do
    if existing_timer, do: Process.cancel_timer(existing_timer)
    Process.send_after(self(), :process_batch, @batch_timeout)
  end

  defp dispatch_batch(paths) do
    files_payload = Enum.reduce(paths, [], fn path, acc ->
      case File.read(path) do
        {:ok, content} -> [%{"path" => path, "content" => content} | acc]
        _ -> acc
      end
    end)

    if length(files_payload) > 0 do
      %{"batch" => files_payload}
      |> Axon.Watcher.IndexingWorker.new()
      |> Oban.insert!()
      
      Logger.info("[Pod A] Enqueued persistent indexing job for #{length(files_payload)} files.")
    end
  end
end
