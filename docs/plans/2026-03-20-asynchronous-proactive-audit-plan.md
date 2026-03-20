# Asynchronous Proactive Audit Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a low-priority, background audit system that calculates accurate Security and Coverage scores for all projects during system idle time, updating the Dashboard asynchronously without blocking ingestion.

**Architecture:** Elixir GenServer (`Axon.Watcher.Server`) detects idle periods (e.g., 5 seconds of inactivity). It sends a message to a new GenServer (`Axon.Watcher.Auditor`). The Auditor fetches projects, sends `axon_audit` JSON-RPC requests via `AxonDashboard.BridgeClient` to the Rust Data Plane, updates the SQLite `IndexedProject` table via `Tracking`, and broadcasts a PubSub event to refresh the `StatusLive` UI.

**Tech Stack:** Elixir, Phoenix PubSub, GenServer, Ecto (SQLite).

---

### Task 1: Idle Detection in Watcher Server

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/server.ex`

**Step 1: Write minimal implementation**
Add an idle timer to `Axon.Watcher.Server` state and handle it.

In `src/dashboard/lib/axon_nexus/axon/watcher/server.ex`:
Update the `init` function to include an `idle_timer` in the state:
```elixir
  def init(%{watch_dir: dir, repo_slug: slug, monitoring_active: active}) do
    # ...
    state = %{
      # ... existing state
      idle_timer: start_idle_timer()
    }
    {:ok, state}
  end
```

Add helper functions:
```elixir
  defp start_idle_timer do
    # 5 seconds of inactivity triggers the idle state
    Process.send_after(self(), :system_idle, 5_000)
  end

  defp reset_idle_timer(timer) do
    if timer, do: Process.cancel_timer(timer)
    start_idle_timer()
  end
```

Update `handle_info` for file events and batch processing to reset the timer:
```elixir
  def handle_info({:file_event, pid, {path, events}}, state) do
    state = %{state | idle_timer: reset_idle_timer(state.idle_timer)}
    # ... existing logic
  end

  def handle_info(:process_batch, state) do
    state = %{state | idle_timer: reset_idle_timer(state.idle_timer)}
    # ... existing logic
  end
```

Add the `handle_info` clause for the idle event:
```elixir
  def handle_info(:system_idle, state) do
    Logger.info("[Pod A] System is idle. Triggering background audit.")
    # Send message to the new Auditor (to be created)
    if Process.whereis(Axon.Watcher.Auditor) do
      send(Axon.Watcher.Auditor, :run_audit)
    end
    # Do NOT restart the timer here. It will restart on the next activity.
    {:noreply, %{state | idle_timer: nil}}
  end
```

**Step 2: Commit**
```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/server.ex
git commit -m "feat(watcher): implement idle detection for background tasks"
```

---

### Task 2: Implement the Auditor GenServer

**Files:**
- Create: `src/dashboard/lib/axon_nexus/axon/watcher/auditor.ex`
- Modify: `src/dashboard/lib/axon_dashboard/application.ex`

**Step 1: Create the Auditor**
Create `src/dashboard/lib/axon_nexus/axon/watcher/auditor.ex`:
```elixir
defmodule Axon.Watcher.Auditor do
  @moduledoc """
  Background worker that runs heavy graph queries (Taint Analysis, Coverage)
  only when the system is idle.
  """
  use GenServer
  require Logger

  def start_link(_) do
    GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  end

  @impl true
  def init(_) do
    {:ok, %{is_auditing: false}}
  end

  @impl true
  def handle_info(:run_audit, %{is_auditing: true} = state) do
    # Already auditing, ignore
    {:noreply, state}
  end

  def handle_info(:run_audit, state) do
    Logger.info("[Auditor] Starting asynchronous background audit...")
    
    # Run in a separate task so we don't block the Auditor GenServer itself
    # although it's fine to block since it only does this one thing.
    # We'll do it synchronously here to prevent overlapping audits easily.
    
    projects = Axon.Watcher.Repo.all(Axon.Watcher.Tracking.IndexedProject)
    
    Enum.each(projects, fn project ->
      Logger.debug("[Auditor] Requesting audit for #{project.name}")
      # Make a synchronous call to the Bridge Client to get the audit
      try do
        response = AxonDashboard.BridgeClient.call_tool("axon_audit", %{"project" => project.name})
        
        case response do
          %{"result" => %{"content" => [%{"text" => text} | _]}} ->
            # Parse the score from the text "Score X/100"
            score = 
              case Regex.run(~r/Score (\d+)\/100/, text) do
                [_, s] -> String.to_integer(s)
                _ -> 100
              end
            
            # Coverage is currently mocked in Rust, but we can query it directly
            # via a custom call if we modify the bridge later, or just assume 0 for now
            # since the real fix requires Rust changes.
            
            Logger.info("[Auditor] Audit complete for #{project.name}: Security Score #{score}")
            Axon.Watcher.Tracking.update_project_scores(project, score, 0)
            
            # Notify LiveView
            Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", :stats_updated)
          _ ->
            Logger.warning("[Auditor] Unexpected response format for #{project.name}")
        end
      catch
        :exit, _ -> Logger.error("[Auditor] Timeout or error querying Bridge for #{project.name}")
      end
    end)
    
    Logger.info("[Auditor] Background audit cycle finished.")
    {:noreply, %{state | is_auditing: false}}
  end
end
```

**Step 2: Add update_project_scores to Tracking**
In `src/dashboard/lib/axon_nexus/axon/watcher/tracking.ex`:
```elixir
  def update_project_scores(project, security_score, coverage_score) do
    project
    |> Ecto.Changeset.change(%{security_score: security_score, coverage_score: coverage_score})
    |> Repo.update()
  end
```

**Step 3: Add to Supervision Tree**
In `src/dashboard/lib/axon_dashboard/application.ex`, add `Axon.Watcher.Auditor` after `Axon.Watcher.StatsCache`:
```elixir
      # ...
      Axon.Watcher.StatsCache,
      Axon.Watcher.Auditor,
      # ...
```

**Step 4: Commit**
```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/auditor.ex src/dashboard/lib/axon_nexus/axon/watcher/tracking.ex src/dashboard/lib/axon_dashboard/application.ex
git commit -m "feat(auditor): implement background graph analysis job"
```

---

### Task 3: Support Synchronous Calls in BridgeClient

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard/bridge_client.ex`

**Step 1: Implement call_tool**
The `BridgeClient` currently only handles async casts or raw TCP. We need a synchronous `GenServer.call` that sends the request and waits for the specific response.

In `src/dashboard/lib/axon_dashboard/bridge_client.ex`:
```elixir
  # Public API
  def call_tool(tool_name, arguments) do
    GenServer.call(__MODULE__, {:call_tool, tool_name, arguments}, 15_000) # 15s timeout
  end

  # Server Callbacks
  def handle_call({:call_tool, tool_name, arguments}, from, %{socket: socket} = state) when not is_nil(socket) do
    id = System.unique_integer([:positive])
    request = %{
      jsonrpc: "2.0",
      method: "tools/call", # Fixed method name to match MCP standard
      params: %{
        name: tool_name,
        arguments: arguments
      },
      id: id
    }
    
    json_payload = Jason.encode!(request) <> "\n"
    :gen_tcp.send(socket, json_payload)
    
    # Store the caller in state so we can reply when the async response arrives
    pending = Map.put(state.pending_calls, id, from)
    {:noreply, %{state | pending_calls: pending}}
  end
  
  def handle_call({:call_tool, _, _}, _from, state) do
    {:reply, {:error, :not_connected}, state}
  end

  # Update init to include pending_calls
  def init(_opts) do
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, connection_status: :disconnected, pending_calls: %{}}}
  end
```

Update `handle_info({:tcp, ...})` to process responses with IDs:
```elixir
  def handle_info({:tcp, _port, data}, state) do
    lines = String.split(data, "\n", trim: true)

    new_state =
      Enum.reduce(lines, state, fn line, acc ->
        if not String.contains?(line, "Axon Bridge Ready") do
          case Jason.decode(line) do
            {:ok, %{"id" => id, "result" => _} = response} when not is_nil(id) ->
              # This is a response to a call_tool
              case Map.pop(acc.pending_calls, id) do
                {nil, _} -> acc # Unknown call
                {from, remaining_calls} ->
                  GenServer.reply(from, response)
                  %{acc | pending_calls: remaining_calls}
              end
            {:ok, %{"id" => id, "error" => _} = response} when not is_nil(id) ->
              case Map.pop(acc.pending_calls, id) do
                {nil, _} -> acc
                {from, remaining_calls} ->
                  GenServer.reply(from, response)
                  %{acc | pending_calls: remaining_calls}
              end
            {:ok, event} ->
              Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})
              acc
            _ ->
              acc
          end
        else
          # Trigger initial scan
          send(Axon.Watcher.Server, :initial_scan)
          acc
        end
      end)

    {:noreply, new_state}
  end
```

**Step 2: Commit**
```bash
git add src/dashboard/lib/axon_dashboard/bridge_client.ex
git commit -m "feat(bridge): implement synchronous JSON-RPC calls over TCP"
```
