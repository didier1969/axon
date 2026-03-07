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
    watch_dir = Keyword.get(opts, :dir, Path.expand("../../../../../", __DIR__))
    Logger.info("Pod A (Watcher) starting supervision on: #{watch_dir}")

    case FileSystem.start_link(dirs: [watch_dir]) do
      {:ok, watcher_pid} ->
        FileSystem.subscribe(watcher_pid)
        {:ok, %{
          watcher_pid: watcher_pid, 
          watch_dir: watch_dir,
          pending_files: MapSet.new(),
          timer: nil
        }}
      
      :ignore ->
        Logger.warning("FileSystem backend not available (e.g. missing inotify-tools).")
        {:ok, %{watcher_pid: nil, watch_dir: watch_dir, pending_files: MapSet.new(), timer: nil}}
        
      {:error, reason} ->
        {:stop, reason}
    end
  end

  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    str_path = to_string(path)
    
    if should_process?(str_path) do
      if :deleted in events do
        Logger.info("[Pod A] Pruning requested for: #{str_path}")
        # Traitement immédiat pour les suppressions
        {:noreply, state}
      else
        # Accumulation pour le parsing (Création / Modification)
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
      
      # On découpe en sous-lots de @max_batch_size
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
    # Pour chaque chemin, on lit le fichier et on prépare le payload
    files_payload = Enum.reduce(paths, [], fn path, acc ->
      case File.read(path) do
        {:ok, content} -> [%{"path" => path, "content" => content} | acc]
        _ -> acc
      end
    end)

    if length(files_payload) > 0 do
      # Lancement asynchrone pour ne pas bloquer le Watcher principal
      Task.start(fn ->
        case PoolFacade.parse_batch(files_payload) do
          {"status", "ok"} -> # MsgPack keys are decoded as binaries/strings
            Logger.info("[Pod C] (Simulated) Ingested batch from Pod B.")
            
          # The exact structure depends on Msgpax decode. Usually maps with string keys.
          %{"status" => "ok", "data" => data} ->
             Logger.info("[Pod C] (Simulated) Ingested #{length(data)} results into HydraDB.")
             
          error ->
            Logger.error("[Pod B] Batch failed: #{inspect(error)}")
        end
      end)
    end
  end
end
