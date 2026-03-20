defmodule Axon.Watcher.StatsCache do
  @moduledoc """
  In-memory GenServer to cache and incrementally update project statistics.
  Prevents the LiveView from hitting the SQLite database with heavy GROUP BY
  queries every second.
  """
  use GenServer
  require Logger

  # Time between full DB syncs (fallback) - e.g., 30 seconds
  @sync_interval 30_000

  def start_link(_) do
    GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  end

  def get_stats do
    GenServer.call(__MODULE__, :get_stats)
  end

  @doc """
  To be called whenever a file is indexed to incrementally update the counters
  without querying the DB.
  """
  def increment_file_stats(project_name, diff) do
    GenServer.cast(__MODULE__, {:increment, project_name, diff})
  end

  @impl true
  def init(_) do
    # Initial load from DB
    send(self(), :sync_from_db)
    {:ok, %{projects: %{}, last_files: []}}
  end

  @impl true
  def handle_call(:get_stats, _from, state) do
    {:reply, state, state}
  end

  @impl true
  def handle_cast({:increment, project_name, diff}, state) do
    # Update the project stats incrementally
    new_projects = Map.update(state.projects, project_name, default_project_stats(diff), fn current ->
      %{
        total: current.total + Map.get(diff, :total, 0),
        completed: current.completed + Map.get(diff, :completed, 0),
        failed: current.failed + Map.get(diff, :failed, 0),
        ignored: current.ignored + Map.get(diff, :ignored, 0),
        symbols: current.symbols + Map.get(diff, :symbols, 0),
        relations: current.relations + Map.get(diff, :relations, 0),
        entries: current.entries + Map.get(diff, :entries, 0),
        security: current.security,
        coverage: current.coverage
      }
    end)

    # Broadcast to LiveView that stats have changed, allowing event-driven UI updates!
    Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", :stats_updated)

    {:noreply, %{state | projects: new_projects}}
  end

  @impl true
  def handle_info(:sync_from_db, state) do
    Logger.debug("[StatsCache] Performing full DB synchronization...")
    try do
      # Load heavy stats from DB once
      stats = Axon.Watcher.Tracking.get_dashboard_stats()
      
      # Schedule next sync
      Process.send_after(self(), :sync_from_db, @sync_interval)
      
      Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", :stats_updated)
      
      {:noreply, %{state | projects: stats.directories, last_files: stats.last_files}}
    catch
      :exit, _ -> 
        Process.send_after(self(), :sync_from_db, @sync_interval)
        {:noreply, state}
    end
  end

  defp default_project_stats(diff) do
    %{
      total: Map.get(diff, :total, 0),
      completed: Map.get(diff, :completed, 0),
      failed: Map.get(diff, :failed, 0),
      ignored: Map.get(diff, :ignored, 0),
      symbols: Map.get(diff, :symbols, 0),
      relations: Map.get(diff, :relations, 0),
      entries: Map.get(diff, :entries, 0),
      security: Map.get(diff, :security, 100),
      coverage: Map.get(diff, :coverage, 0)
    }
  end
end