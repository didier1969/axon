defmodule Axon.Watcher.Server do
  @moduledoc """
  The core orchestrator of Pod A.
  Watches the filesystem, batches events, and dispatches to the Worker Pool.
  Now with strict Binary Filtering to prevent Ecto/Oban crashes.
  """
  use GenServer
  require Logger

  @batch_timeout 500

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
    repo_slug = System.get_env("AXON_REPO_SLUG") || Path.expand(".") |> Path.basename()
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
      idle_timer: start_idle_timer()
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
    Logger.info("[Pod A] AUTO-START: Waiting for manual or Rust-led scan...")

    # Phoenix.PubSub.broadcast(
    #   AxonDashboard.PubSub,
    #   "bridge_events",
    #   {:scan_started, state.watch_dir}
    # )

    # send(self(), :initial_scan)
    # Scan is now handled by Rust Data Plane.
    # Axon.Watcher.PoolFacade.trigger_global_scan()

    # Schedule automated retry for failed files every 5 minutes
    # :timer.send_interval(300_000, self(), :retry_failed)

    {:noreply, state}
  end

  @impl true
  def handle_cast(:trigger_scan, state) do
    # send(self(), :initial_scan)
    # Scan is now handled by Rust Data Plane.
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
  def handle_call(:get_monitoring_status, _from, state),
    do: {:reply, state.monitoring_active, state}

  @impl true
  def handle_info(:retry_failed, state) do
    failed_files = Axon.Watcher.Tracking.get_failed_files(100)

    if length(failed_files) > 0 do
      Logger.info("[Pod A] Retrying #{length(failed_files)} failed files...")

      Enum.each(failed_files, fn str_path ->
        try do
          Axon.Watcher.Tracking.mark_file_status!(str_path, "pending")
        rescue
          _ -> :ok
        end
      end)

      dispatch_batch(failed_files, :indexing_hot)
    end

    {:noreply, state}
  end

  @impl true
  def handle_info({:ok, "done"}, state) do
    Logger.info("[Pod A] Reactive Scan Completed.")
    Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", progress: 100})
    {:noreply, state}
  end

  @impl true
  def handle_info({:ok, path}, state) do
    str_path = to_string(path)

    project_name = get_top_dir(str_path, state.watch_dir)
    project_path = Path.expand(project_name, state.watch_dir)

    try do
      Axon.Watcher.Tracking.upsert_project!(project_name, project_path)
    rescue
      _ -> :ok
    end

    if should_process?(str_path) do
      priority = calculate_priority(str_path)

      case File.stat(str_path) do
        {:ok, %{mtime: mtime}} ->
          last_mtime = Axon.Watcher.Tracking.get_file_hash(str_path)
          current_mtime = :erlang.phash2(mtime)

          # Optimization: Only mark as pending and enqueue if the file CHANGED 
          # or if it's not yet successfully indexed in the local DB.
          current_status = Axon.Watcher.Tracking.get_file_status(str_path)

          if last_mtime == nil or current_mtime != last_mtime or
               current_status not in ["indexed", "ignored_by_rule"] do
            
            # TITAN PROTOCOL: Route massive files (>1MB) to the single-threaded queue immediately.
            case File.stat(str_path) do
              {:ok, %{size: size}} when size > 1_048_576 ->
                # Titan stays synchronous for now as they are rare
                # We still need to upsert here if we bypass staging
                Axon.Watcher.Tracking.upsert_file!(project_name, str_path, current_mtime, "pending")
                dispatch_batch([str_path], :indexing_titan)
                {:noreply, state}

              _ ->
                # NEXUS PRODUCTION: All other files go to ETS for batched insertion
                Axon.Watcher.Staging.stage_file(project_name, str_path, current_mtime, priority)
                {:noreply, state}
            end
          else
            # File is already indexed and hasn't changed on disk.
            {:noreply, state}
          end

        _ ->
          {:noreply, state}
      end
    else
      {:noreply, state}
    end
  end

  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    state = %{state | idle_timer: reset_idle_timer(state.idle_timer)}
    str_path = to_string(path)

    if state.monitoring_active and should_process?(str_path) do
      if :deleted in events do
        # Dans le futur, on notifiera la suppression au Pod C. Pour l'instant on l'ignore.
        {:noreply, state}
      else
        # UNIQUEMENT réindexer le fichier modifié (suppression du "neighbor scan" causant des boucles infinies)
        new_pending = MapSet.put(state.pending_files, str_path)

        new_timer = reset_timer(state.timer)
        {:noreply, %{state | pending_files: new_pending, timer: new_timer}}
      end
    else
      {:noreply, state}
    end
  end

  @impl true
  def handle_info(:initial_scan, state) do
    Logger.info("[Pod A] Triggering Reactive Streaming Scan on: #{state.watch_dir}")

    # Mark all existing files as stale. True orphan files will remain stale and can be cleaned up later.
    Axon.Watcher.Repo.query!("UPDATE indexed_files SET status = 'stale'")

    Axon.Watcher.Progress.update_status(state.repo_slug, %{
      status: "indexing",
      total: 0,
      progress: 0
    })

    # Déclenche le scan asynchrone qui enverra des messages {:ok, path} ou {:ok, "done"}
    Axon.Scanner.start_streaming(state.watch_dir, self())
    {:noreply, state}
  end

  @impl true
  def handle_info(:process_batch, state) do
    state = %{state | idle_timer: reset_idle_timer(state.idle_timer)}
    files_to_process = MapSet.to_list(state.pending_files)

    if length(files_to_process) > 0 do
      Enum.each(files_to_process, fn path ->
        priority = calculate_priority(path)
        mtime = case File.stat(path) do
          {:ok, %{mtime: t}} -> :erlang.phash2(t)
          _ -> 0
        end
        Axon.Watcher.Staging.stage_file("event", path, mtime, priority)
      end)
    end

    {:noreply, %{state | pending_files: MapSet.new(), timer: nil}}
  end

  @impl true
  def handle_info(:system_idle, state) do
    Logger.info("[Pod A] System is idle. Triggering background audit.")
    # Send message to the new Auditor (to be created)
    if Process.whereis(Axon.Watcher.Auditor) do
      send(Axon.Watcher.Auditor, :run_audit)
    end

    # Do NOT restart the timer here. It will restart on the next activity.
    {:noreply, %{state | idle_timer: nil}}
  end

  defp should_process?(path) do
    # BARE MINIMUM "ANTI-AVALANCHE" SHIELD
    # This prevents the Erlang VM from being flooded by 10,000+ Inotify events during builds/deps installs.
    # All other domain filtering (extensions, specific ignore rules) should be handled dynamically via .axonignore logic.
    not (String.contains?(path, "/.git/") or
           String.contains?(path, "/.axon/") or
           String.contains?(path, "/_build/") or
           String.contains?(path, "/deps/") or
           String.contains?(path, "/.devenv/") or
           String.contains?(path, "/node_modules/") or
           String.contains?(path, "/target/"))
  end

  defp reset_timer(existing_timer) do
    if existing_timer, do: Process.cancel_timer(existing_timer)
    Process.send_after(self(), :process_batch, @batch_timeout)
  end

  defp start_idle_timer do
    # 5 seconds of inactivity triggers the idle state
    Process.send_after(self(), :system_idle, 5_000)
  end

  defp reset_idle_timer(timer) do
    if timer, do: Process.cancel_timer(timer)
    start_idle_timer()
  end

  defp dispatch_batch(paths, queue) do
    # Optimization: we don't read file content here to avoid blocking the GenServer
    # and to prevent blowing up the Erlang RAM and Oban DB size.
    files_payload =
      Enum.map(paths, fn path ->
        %{
          "path" => path,
          "trace_id" => Ecto.UUID.generate(),
          "t0" => :os.system_time(:microsecond)
        }
      end)

    if length(files_payload) > 0 do
      try do
        # On passe explicitement une Map à Oban
        job_args = %{"batch" => files_payload}

        Axon.Watcher.IndexingWorker.new(job_args, queue: queue)
        |> Oban.insert!()

        :telemetry.execute([:axon, :watcher, :batch_enqueued], %{count: length(files_payload)}, %{
          queue: queue
        })

        Logger.info("[Pod A] Enqueued batch of #{length(files_payload)} files to #{queue}.")
      rescue
        e ->
          :telemetry.execute([:axon, :watcher, :batch_failed], %{count: length(files_payload)}, %{
            queue: queue,
            error: inspect(e)
          })

          Logger.error("[Pod A] FAILED to enqueue batch: #{inspect(e)}")
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

  defp get_top_dir(path, watch_dir) do
    # On force la résolution absolue pour la sécurité
    abs_path = Path.expand(path)
    abs_watch_dir = Path.expand(watch_dir)

    # On vérifie que le fichier est bien DANS le watch_dir
    if String.starts_with?(abs_path, abs_watch_dir) do
      # On soustrait le watch_dir au chemin complet
      relative_path =
        abs_path
        |> String.replace_prefix(abs_watch_dir, "")
        |> String.trim_leading("/")

      # On prend le premier dossier de ce chemin relatif (le nom du projet)
      case Path.split(relative_path) do
        [dir | _] when dir != "." and dir != "" -> dir
        _ -> "root"
      end
    else
      "external"
    end
  end
end
