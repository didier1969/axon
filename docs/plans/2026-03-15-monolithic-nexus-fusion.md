# Axon Nexus Monolithic Fusion Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Merge the Elixir Watcher (Pod A) and Elixir Dashboard (Control Plane) into a single Phoenix application (`axon_dashboard`) to eliminate Erlang clustering fragility and ensure real-time local PubSub updates.

**Architecture:** We will copy the Watcher's modules, dependencies, and configuration into the Dashboard. The Dashboard's `application.ex` will start both the Phoenix Web server and the Watcher's supervision tree (Scanner, Oban, etc.). The BridgeClient will handle socket communication with Rust.

**Tech Stack:** Elixir, Phoenix, Oban, Rustler (NIF).

---

### Task 1: Merge Dependencies and Configuration

**Files:**
- Modify: `src/dashboard/mix.exs`
- Modify: `src/dashboard/config/config.exs`
- Modify: `src/dashboard/config/dev.exs`

**Step 1: Add Watcher dependencies to Dashboard `mix.exs`**

Add `:rustler`, `:file_system`, `:ecto_sqlite3`, and `:oban` to `deps/0`.

```elixir
      {:rustler, "~> 0.36.0", runtime: false},
      {:file_system, "~> 1.0"},
      {:ecto_sqlite3, "~> 0.10"},
      {:oban, "~> 2.18"}
```

**Step 2: Add Ecto setup to `mix.exs` aliases**

Add to `aliases/0`:
```elixir
      "ecto.setup": ["ecto.create", "ecto.migrate", "run priv/repo/seeds.exs"],
      "ecto.reset": ["ecto.drop", "ecto.setup"],
```

**Step 3: Update `config/config.exs`**

Add Oban and Repo configuration:

```elixir
config :axon_dashboard,
  ecto_repos: [Axon.Watcher.Repo],
  generators: [timestamp_type: :utc_datetime]

config :axon_dashboard, Axon.Watcher.Repo,
  database: "axon_nexus.db",
  pool_size: 5

config :axon_dashboard, Oban,
  repo: Axon.Watcher.Repo,
  engine: Oban.Engines.Lite,
  plugins: [Oban.Plugins.Pruner],
  queues: [
    indexing_critical: [limit: 10],
    indexing_hot: [limit: 5],
    indexing_default: [limit: 10]
  ]
```

**Step 4: Commit**

```bash
git add src/dashboard/mix.exs src/dashboard/config/config.exs src/dashboard/config/dev.exs
git commit -m "chore(nexus): merge Watcher dependencies and config into Dashboard"
```

---

### Task 2: Port Watcher Code to Dashboard

**Files:**
- Execute commands to copy files.

**Step 1: Copy Watcher lib, native, and migrations**

Run these shell commands (already partially done, but ensure correctness):
```bash
cp -r src/watcher/lib/axon src/dashboard/lib/
cp -r src/watcher/native src/dashboard/
mkdir -p src/dashboard/priv/repo/migrations
cp src/watcher/priv/repo/migrations/* src/dashboard/priv/repo/migrations/
```

**Step 2: Fix Rustler NIF loading path**

In `src/dashboard/lib/axon/scanner.ex`, change `:axon_watcher` to `:axon_dashboard`.

```elixir
defmodule Axon.Scanner do
  use Rustler, otp_app: :axon_dashboard, crate: "axon_scanner"
```

**Step 3: Commit**

```bash
git add src/dashboard/lib/axon src/dashboard/native src/dashboard/priv/repo
git commit -m "feat(nexus): port Watcher source code and NIFs to Dashboard app"
```

---

### Task 3: Unify the Supervision Tree

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard/application.ex`

**Step 1: Remove Erlang Clustering logic**

Remove `Node.connect(:"watcher@127.0.0.1")`.

**Step 2: Integrate Watcher children**

Add the Watcher processes to the Dashboard's children list. Remove duplicate PubSub. Use `AxonDashboard.PubSub` everywhere.

```elixir
    children = [
      AxonDashboardWeb.Telemetry,
      Axon.Watcher.Repo, # Added
      {Oban, Application.fetch_env!(:axon_dashboard, Oban)}, # Added
      {Axon.Watcher.Server, []}, # Added
      {DNSCluster, query: Application.get_env(:axon_dashboard, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: AxonDashboard.PubSub},
      AxonDashboard.BridgeClient,
      AxonDashboardWeb.Endpoint
    ]
```

**Step 3: Commit**

```bash
git add src/dashboard/lib/axon_dashboard/application.ex
git commit -m "feat(nexus): unify supervision tree with Watcher processes"
```

---

### Task 4: Fix PubSub References

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard_web/live/status_live.ex`
- Modify: `src/dashboard/lib/axon/watcher/server.ex`
- Modify: `src/dashboard/lib/axon/watcher/indexing_worker.ex`

**Step 1: Fix `server.ex`**

Replace `Axon.PubSub` with `AxonDashboard.PubSub`.

```elixir
Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:scan_started, state.watch_dir})
```

**Step 2: Fix `indexing_worker.ex`**

Remove PoolFacade broadcasting (since BridgeClient does it from Rust, or we can just keep BridgeClient). Wait, since they are in the same app, `BridgeClient` handles `WATCHER_EVENT` from Rust and broadcasts `{:file_indexed, path, status}` to `AxonDashboard.PubSub` on `"bridge_events"`. No changes strictly needed in worker if it uses `PoolFacade.broadcast_event`. Let's update `PoolFacade` to ensure it works.

Actually, update `indexing_worker.ex` to use `AxonDashboard.PoolFacade` if we move it, or just keep it as `Axon.Watcher.PoolFacade`.

Ensure `src/dashboard/lib/axon/watcher/pool_facade.ex` is in the supervision tree!
Add `Axon.Watcher.PoolFacade` to `src/dashboard/lib/axon_dashboard/application.ex` children list (before `Axon.Watcher.Server`).

**Step 3: Fix `status_live.ex`**

Remove clustering checks and RPC PubSub subscriptions. The Dashboard just subscribes to `"bridge_events"` locally.

```elixir
  def mount(_params, _session, socket) do
    if connected?(socket) do
      :timer.send_interval(1000, self(), :tick)
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
    end

    # Remove cluster_connected logic...
    {:ok, assign(socket, cluster_connected: true, ...)}
  end

  def handle_info(:tick, socket) do
    {:noreply, assign(socket, sys_time: Time.utc_now() |> Time.truncate(:second))}
  end
```

**Step 4: Commit**

```bash
git add src/dashboard/lib
git commit -m "fix(nexus): unify PubSub and remove distributed clustering logic"
```

---

### Task 5: Streamline Startup Script

**Files:**
- Modify: `scripts/start-v2.sh`
- Modify: `devenv.nix`

**Step 1: Update `devenv.nix`**

Remove the `watcher` process. Rename `dashboard` to `nexus`.

```nix
  processes = {
    db.exec = "axon-db-start";
    core.exec = "/home/dstadel/projects/axon/bin/axon-core";
    
    nexus.exec = ''
      export ELIXIR_HOME="$PWD/.axon/elixir_home"
      export MIX_HOME="$ELIXIR_HOME/mix"
      export HEX_HOME="$ELIXIR_HOME/hex"
      export PATH="$MIX_HOME/bin:$HEX_HOME/bin:$PATH"
      cd src/dashboard && mix ecto.setup && PHX_PORT=44921 AXON_REPO_SLUG=axon AXON_WATCH_DIR=../../ mix phx.server
    '';
  };
```

**Step 2: Update `scripts/start-v2.sh`**

Simplify TMUX windows.
```bash
# Start Pod A/Control (Nexus)
tmux new-window -t axon:2 -n "nexus" "bash -c 'cd src/dashboard && nix develop --impure --command bash -c \"mix ecto.setup && PHX_PORT=44921 AXON_REPO_SLUG=axon AXON_WATCH_DIR=../../ mix phx.server\"'"
```
Remove window 3 (Dashboard).

**Step 3: Commit**

```bash
git add scripts/start-v2.sh devenv.nix
git commit -m "chore(nexus): streamline startup script to use monolithic Nexus app"
```
