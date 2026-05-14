# Axon Live Cockpit v2.0 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform the static dashboard into a real-time Cockpit that visualizes active file ingestion using Erlang clustering and PubSub.

**Architecture:** Configure Erlang node clustering between Pod A (Watcher) and Dashboard. Expose Watcher events via `Phoenix.PubSub` and consume them in `AxonDashboardWeb.StatusLive`. Add a "Matrix" view for incoming files.

**Tech Stack:** Elixir, Phoenix LiveView, Erlang Distribution (`Node`), Phoenix.PubSub.

---

### Task 1: Enable Erlang Clustering via DevEnv

**Files:**
- Modify: `devenv.nix`

**Step 1: Update process commands**

Modify the `watcher` and `dashboard` processes in `devenv.nix` to use named nodes and the shared cookie.

```nix
    watcher.exec = ''
      export PYTHONPATH="$PYTHONPATH:$PWD/src"
      export ELIXIR_HOME="$PWD/.axon/elixir_home"
      export MIX_HOME="$ELIXIR_HOME/mix"
      export HEX_HOME="$ELIXIR_HOME/hex"
      export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"
      cd src/watcher && mix ecto.setup && AXON_REPO_SLUG=axon AXON_WATCH_DIR="/home/dstadel/projects/axon" elixir --sname watcher@localhost --cookie axon_v2_cluster -S mix run --no-halt
    '';

    dashboard.exec = ''
      export ELIXIR_HOME="$PWD/.axon/elixir_home"
      export MIX_HOME="$ELIXIR_HOME/mix"
      export HEX_HOME="$ELIXIR_HOME/hex"
      export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"
      cd src/dashboard && PHX_PORT=44921 elixir --sname dashboard@localhost --cookie axon_v2_cluster -S mix phx.server
    '';
```

**Step 2: Commit**

```bash
git add devenv.nix
git commit -m "chore(infra): enable Erlang clustering for Watcher and Dashboard"
```

---

### Task 2: Connect Dashboard to Watcher Cluster

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard/application.ex`

**Step 1: Add Node.connect logic on startup**

```elixir
  def start(_type, _args) do
    # Attempt to connect to the Watcher node
    Node.connect(:"watcher@localhost")
    
    children = [
      ...
```

**Step 2: Commit**

```bash
git add src/dashboard/lib/axon_dashboard/application.ex
git commit -m "feat(dashboard): connect to Watcher Erlang node on startup"
```

---

### Task 3: Setup Shared PubSub in Watcher

**Files:**
- Modify: `src/watcher/lib/axon/watcher/application.ex`
- Modify: `src/watcher/lib/axon/watcher/server.ex`
- Modify: `src/watcher/lib/axon/watcher/indexing_worker.ex`

**Step 1: Start PubSub in Watcher's Application**

```elixir
    children = [
      {Phoenix.PubSub, name: Axon.PubSub},
      ...
```

**Step 2: Broadcast Scan Started**

In `src/watcher/lib/axon/watcher/server.ex`:
```elixir
  def handle_continue(:auto_trigger_scan, state) do
    Logger.info("[Pod A] AUTO-START: Triggering initial scan...")
    Phoenix.PubSub.broadcast(Axon.PubSub, "watcher_events", {:scan_started})
    send(self(), :initial_scan)
    {:noreply, state}
  end
```

**Step 3: Broadcast File Indexed**

In `src/watcher/lib/axon/watcher/indexing_worker.ex`:
```elixir
      case PoolFacade.parse(file["path"], file["content"]) do
        %{"status" => "ok"} ->
          Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", file["path"], :ok)
          Phoenix.PubSub.broadcast(Axon.PubSub, "watcher_events", {:file_indexed, file["path"], :ok})
```

**Step 4: Commit**

```bash
git add src/watcher/lib/axon/watcher/application.ex src/watcher/lib/axon/watcher/server.ex src/watcher/lib/axon/watcher/indexing_worker.ex
git commit -m "feat(watcher): broadcast indexing events via global PubSub"
```

---

### Task 4: Refactor StatusLive for Matrix View

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard_web/live/status_live.ex`

**Step 1: Update mount and subscribe**

```elixir
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to BOTH bridge and watcher
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
      # If cluster is connected, this works automatically
      Phoenix.PubSub.subscribe(Axon.PubSub, "watcher_events")
    end

    {:ok, assign(socket, 
      cluster_connected: Node.ping(:"watcher@localhost") == :pong,
      live_files: [], # [{path, status}] limited to 20
      total_files_parsed: 0,
      # ... other existing assigns
    )}
  end
```

**Step 2: Handle Watcher Events**

```elixir
  def handle_info({:scan_started}, socket) do
    {:noreply, assign(socket, status: :processing, live_files: [])}
  end

  def handle_info({:file_indexed, path, status}, socket) do
    new_files = [{path, status} | socket.assigns.live_files] |> Enum.take(20)
    {:noreply, assign(socket, 
      live_files: new_files,
      total_files_parsed: socket.assigns.total_files_parsed + 1
    )}
  end
```

**Step 3: Add Matrix UI (in `render/1`)**

Add a new section at the bottom for real-time logs.

```html
      <!-- Matrix View: Live Ingestion Log -->
      <div class="mt-12 premium-card p-6 bg-black">
        <div class="flex justify-between items-center mb-4">
          <h3 class="text-sm font-bold text-white uppercase tracking-widest flex items-center gap-2">
            <div class={"w-2 h-2 rounded-full #{if @cluster_connected, do: "bg-green-500 animate-pulse", else: "bg-red-500"}"}></div>
            Neural Link (Erlang Cluster)
          </h3>
          <span class="text-xs font-mono text-primary"><%= @total_files_parsed %> FILES INGESTED</span>
        </div>
        <div class="h-64 overflow-y-auto font-mono text-xs space-y-1 flex flex-col-reverse">
          <%= for {file, status} <- @live_files do %>
            <div class="flex items-center gap-4">
              <span class="opacity-50">>_</span>
              <span class={"font-bold #{if status == :ok, do: "text-green-500", else: "text-red-500"}"}">
                [<%= if status == :ok, do: "OK", else: "ERR" %>]
              </span>
              <span class="text-green-400 opacity-80"><%= file %></span>
            </div>
          <% end %>
        </div>
      </div>
```

**Step 4: Commit**

```bash
git add src/dashboard/lib/axon_dashboard_web/live/status_live.ex
git commit -m "feat(dashboard): add matrix view for real-time file ingestion"
```
