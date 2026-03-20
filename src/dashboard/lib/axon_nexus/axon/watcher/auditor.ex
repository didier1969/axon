defmodule Axon.Watcher.Auditor do
  @moduledoc """
  OTP Debouncer. Absorbs high-frequency file ingestion events and triggers
  a low-frequency heavy graph audit to update dashboard security scores.
  """
  use GenServer
  require Logger

  @idle_timeout 3_000    # Wait 3s after the LAST file to audit
  @max_delay 30_000      # Force audit at least every 30s during continuous ingestion

  def start_link(_) do
    GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  end

  @impl true
  def init(_) do
    Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
    {:ok, %{idle_timer: nil, max_delay_timer: nil, pending_changes: false}}
  end

  @impl true
  def handle_info({:bridge_event, {:file_indexed, _path, _status}}, state) do
    if state.idle_timer, do: Process.cancel_timer(state.idle_timer)
    
    new_idle = Process.send_after(self(), :trigger_heavy_audit, @idle_timeout)
    new_max = state.max_delay_timer || Process.send_after(self(), :trigger_heavy_audit, @max_delay)

    {:noreply, %{state | idle_timer: new_idle, max_delay_timer: new_max, pending_changes: true}}
  end

  # Handle the audit completion from BridgeClient
  def handle_info({:bridge_event, %{"id" => id, "result" => %{"content" => [%{"text" => text} | _]}}}, state) when is_integer(id) do
    # Hacky but effective parser for the MCP string output
    score = 
      case Regex.run(~r/Score (\d+)\/100/, text) do
        [_, s] -> String.to_integer(s)
        _ -> 100
      end
      
    # Extract project name from the response context (we can extract from the string for now)
    project_name = 
      case Regex.run(~r/Security Audit for ([^:]+):/, text) do
        [_, p] -> String.trim(p)
        _ -> nil
      end

    if project_name do
      Logger.info("[Auditor] Proactive Audit completed for #{project_name}. Score: #{score}")
      
      # Update tracking database
      if project = Axon.Watcher.Repo.get_by(Axon.Watcher.Tracking.IndexedProject, name: project_name) do
         Axon.Watcher.Tracking.update_project_scores(project, score, 0) # Coverage 0 for now
         # Invalidate StatsCache
         send(Axon.Watcher.StatsCache, :sync_from_db)
      end
    end
    
    {:noreply, state}
  end

  def handle_info(:trigger_heavy_audit, state) do
    if state.pending_changes do
      Logger.info("[Auditor] System idle. Triggering asynchronous Taint Analysis.")
      
      projects = Axon.Watcher.Repo.all(Axon.Watcher.Tracking.IndexedProject)
      Enum.each(projects, fn p -> 
         AxonDashboard.BridgeClient.trigger_async_audit(p.name)
      end)
    end

    if state.idle_timer, do: Process.cancel_timer(state.idle_timer)
    if state.max_delay_timer, do: Process.cancel_timer(state.max_delay_timer)

    {:noreply, %{state | idle_timer: nil, max_delay_timer: nil, pending_changes: false}}
  end
  
  def handle_info(_, state), do: {:noreply, state}
end
