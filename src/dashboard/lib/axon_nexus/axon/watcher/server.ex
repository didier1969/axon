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
    Logger.info("[Pod A] Forwarding manual scan request to Rust Data Plane.")
    Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "indexing", progress: 0})
    :telemetry.execute([:axon, :watcher, :manual_scan_triggered], %{count: 1}, %{
      repo_slug: state.repo_slug,
      watch_dir: state.watch_dir
    })
    Axon.Watcher.PoolFacade.trigger_global_scan()
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

      Axon.Watcher.BatchDispatch.dispatch(failed_files, :indexing_hot)
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

    {:noreply, process_discovered_file(str_path, state)}
  end

  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    state = %{state | idle_timer: reset_idle_timer(state.idle_timer)}
    str_path = to_string(path)

    if state.monitoring_active and Axon.Watcher.PathPolicy.should_process?(str_path) do
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
        priority = Axon.Watcher.PathPolicy.calculate_priority(path)
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

  defp process_discovered_file(str_path, state) do
    project_name = Axon.Watcher.PathPolicy.get_top_dir(str_path, state.watch_dir)
    ensure_project(project_name, state.watch_dir)

    if Axon.Watcher.PathPolicy.should_process?(str_path) do
      maybe_enqueue_discovered_file(str_path, project_name)
    end

    state
  end

  defp ensure_project(project_name, watch_dir) do
    project_path = Path.expand(project_name, watch_dir)

    try do
      Axon.Watcher.Tracking.upsert_project!(project_name, project_path)
    rescue
      _ -> :ok
    end
  end

  defp maybe_enqueue_discovered_file(str_path, project_name) do
    with {:ok, %{mtime: mtime}} <- File.stat(str_path) do
      current_mtime = :erlang.phash2(mtime)

      if should_reindex_file?(str_path, current_mtime) do
        route_discovered_file(project_name, str_path, current_mtime)
      end
    end
  end

  defp should_reindex_file?(str_path, current_mtime) do
    last_mtime = Axon.Watcher.Tracking.get_file_hash(str_path)
    current_status = Axon.Watcher.Tracking.get_file_status(str_path)

    last_mtime == nil or current_mtime != last_mtime or
      current_status not in ["indexed", "ignored_by_rule"]
  end

  defp route_discovered_file(project_name, str_path, current_mtime) do
    priority = Axon.Watcher.PathPolicy.calculate_priority(str_path)

    case File.stat(str_path) do
      {:ok, %{size: size}} when size > 1_048_576 ->
        Axon.Watcher.Tracking.upsert_file!(project_name, str_path, current_mtime, "pending")
        Axon.Watcher.BatchDispatch.dispatch([str_path], :indexing_titan)

      _ ->
        Axon.Watcher.Staging.stage_file(project_name, str_path, current_mtime, priority)
    end
  end

end
