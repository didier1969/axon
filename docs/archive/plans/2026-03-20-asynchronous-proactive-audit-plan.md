# Asynchronous Proactive Audit Implementation Plan (Google Experts Edition)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a scalable, high-performance background audit system that calculates accurate Security (Taint) and Coverage scores without blocking the 70% CPU-capped hot ingestion path.

**Architecture (Based on Google SWE analysis):**
1. **Rust Cypher Optimization:** Reverse the depth-4 KuzuDB traversal to anchor on the low-cardinality "Sinks" (eval/unsafe) instead of expanding all symbols forward.
2. **Rust Tokio Executor Unblocking:** Move the 1.7s synchronous `McpServer::handle_request` FFI calls off the main async event loop into `tokio::task::spawn_blocking` to prevent Head-of-Line (HoL) blocking of UDS IPC.
3. **Elixir Debouncer (Auditor):** Use a pure OTP GenServer with an Idle Timer and a Max-Delay Circuit Breaker to collapse high-frequency file events into a single async audit trigger.

**Tech Stack:** Rust (Tokio spawn_blocking), KuzuDB (Cypher), Elixir (OTP GenServer, PubSub).

---

### Task 1: Optimize Cypher Queries in Rust Data Plane

**Files:**
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Rewrite get_security_audit**
Reverse the `MATCH` traversal and optimize the `WHERE` clause.

```rust
    pub fn get_security_audit(&self, project_name: &str) -> Result<(usize, String)> {
        // Taint analysis: Path from any dangerous sink BACKWARDS to a symbol in the file
        let count_query = format!(
            "MATCH (d:Symbol)<-[:CALLS|CALLS_NIF*1..4]-(s:Symbol)<-[:CONTAINS]-(f:File) 
             WHERE (d.name IN ['eval', 'exec', 'system', 'pickle'] OR d.is_unsafe = true) AND f.path CONTAINS '{}' 
             RETURN count(DISTINCT s)",
            project_name
        );
        let issues = self.query_count(&count_query)?;
        
        let score = if issues > 0 {
            (100 - (issues * 15).min(100)) as usize
        } else {
            100
        };

        let paths_query = format!(
            "MATCH path = (d:Symbol)<-[:CALLS|CALLS_NIF*1..4]-(s:Symbol)<-[:CONTAINS]-(f:File) 
             WHERE (d.name IN ['eval', 'exec', 'system', 'pickle'] OR d.is_unsafe = true) AND f.path CONTAINS '{}' 
             RETURN path LIMIT 5",
            project_name
        );
        
        let paths_json = self.query_json(&paths_query).unwrap_or_else(|_| "[]".to_string());
        
        Ok((score, paths_json))
    }
```

**Step 2: Commit**
```bash
git add src/axon-core/src/graph.rs
git commit -m "perf(graph): reverse taint analysis cypher query for index optimization"
```

---

### Task 2: Unblock Tokio UDS Loop in Rust

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Use tokio::spawn and spawn_blocking for MCP**
Find the `} else if command.starts_with('{') {` block in the UDS listener loop.

```rust
                } else if command.starts_with('{') {
                    // MCP Request - Offload heavy graph queries from Tokio worker thread
                    let store_for_mcp = store_clone.clone();
                    let command_clone = command.to_string();
                    let tx_clone = tx.clone();
                    
                    tokio::spawn(async move {
                        let mcp_server = McpServer::new(store_for_mcp);
                        if let Ok(request) = serde_json::from_str::<mcp::JsonRpcRequest>(&command_clone) {
                            
                            // Execute synchronous FFI graph query in blocking thread pool
                            let response = tokio::task::spawn_blocking(move || {
                                mcp_server.handle_request(request)
                            }).await.expect("Blocking MCP task panicked");
                            
                            if let Ok(json_str) = serde_json::to_string(&response) {
                                let _ = tx_clone.send(format!("{}\n", json_str)).await;
                            }
                        }
                    });
                }
```

**Step 2: Commit**
```bash
git add src/axon-core/src/main.rs
git commit -m "perf(core): offload synchronous MCP graph queries to tokio blocking pool"
```

---

### Task 3: The Elixir Debouncer (Auditor)

**Files:**
- Create: `src/dashboard/lib/axon_nexus/axon/watcher/auditor.ex`
- Modify: `src/dashboard/lib/axon_dashboard/application.ex`

**Step 1: Write Axon.Watcher.Auditor**
Create `src/dashboard/lib/axon_nexus/axon/watcher/auditor.ex`:

```elixir
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
```

**Step 2: Add to Application Tree**
In `src/dashboard/lib/axon_dashboard/application.ex`, add `Axon.Watcher.Auditor` after `Axon.Watcher.StatsCache`:
```elixir
      # ...
      Axon.Watcher.StatsCache,
      Axon.Watcher.Auditor,
      # ...
```

**Step 3: Add Helper to Tracking**
In `src/dashboard/lib/axon_nexus/axon/watcher/tracking.ex`:
```elixir
  def update_project_scores(project, security_score, coverage_score) do
    project
    |> Ecto.Changeset.change(%{security_score: security_score, coverage_score: coverage_score})
    |> Repo.update()
  end
```

**Step 4: Commit**
```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/auditor.ex src/dashboard/lib/axon_dashboard/application.ex src/dashboard/lib/axon_nexus/axon/watcher/tracking.ex
git commit -m "feat(auditor): implement otp debouncer for proactive security audits"
```

---

### Task 4: Connect the Async Bridge

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard/bridge_client.ex`

**Step 1: Add trigger_async_audit/1**
We just need to cast an MCP message blindly. The response will come back over the TCP loop with the same ID, and since it's a valid JSON-RPC response, it will be broadcasted to `bridge_events`, which `Auditor` listens to.

```elixir
  # Public API
  def trigger_async_audit(project_name) do
    GenServer.cast(__MODULE__, {:async_audit, project_name})
  end

  # Server Callbacks
  def handle_cast({:async_audit, project_name}, %{socket: socket} = state) when not is_nil(socket) do
    # Using a unique ID pattern that the Auditor can recognize (e.g., above 10000)
    id = System.unique_integer([:positive]) + 10000
    
    request = %{
      jsonrpc: "2.0",
      method: "tools/call",
      params: %{
        name: "axon_audit",
        arguments: %{"project" => project_name}
      },
      id: id
    }
    
    json_payload = Jason.encode!(request) <> "\n"
    :gen_tcp.send(socket, json_payload)
    
    {:noreply, state}
  end
  def handle_cast({:async_audit, _}, state), do: {:noreply, state}
```

*Note: Ensure `handle_info({:tcp, ...})` in BridgeClient already decodes JSON and broadcasts `{:bridge_event, event}` for all unhandled JSON responses (which it does via the wildcard match).*

**Step 2: Commit**
```bash
git add src/dashboard/lib/axon_dashboard/bridge_client.ex
git commit -m "feat(bridge): dispatch async audit MCP commands without blocking"
```
