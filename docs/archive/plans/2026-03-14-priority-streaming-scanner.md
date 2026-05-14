# Axon Priority Streaming Scanner (APSS) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a real-time, priority-aware file scanner using a detached Rust thread and Elixir message passing.

**Architecture:** A Rust NIF starts a background thread using the `ignore` crate. This thread sends `{:file_discovered, path}` messages to the Elixir GenServer, which scores them and dispatches them to specialized Oban queues.

**Tech Stack:** Rust (rustler, ignore), Elixir (GenServer, Oban).

---

### Task 1: Update Rust Scanner to support Async Messaging

**Files:**
- Modify: `src/watcher/native/axon_scanner/src/lib.rs`
- Test: `src/watcher/native/axon_scanner/Cargo.toml` (Check dependencies)

**Step 1: Update Cargo.toml with crossbeam-channel (for thread safety if needed)**

```toml
[dependencies]
rustler = "0.36.1"
ignore = "0.4.22"
walkdir = "2"
```

**Step 2: Implement `start_streaming` NIF in `lib.rs`**

```rust
use rustler::{Env, ResourceArc, Term, Encoder, OwnedEnv};
use std::thread;
use ignore::WalkBuilder;
use std::path::Path;

#[rustler::nif]
fn start_streaming(env: Env, path: String, pid: rustler::LocalPid) -> rustler::Atom {
    let mut owned_env = OwnedEnv::new();
    let root_path = path.clone();

    thread::spawn(move || {
        let mut builder = WalkBuilder::new(Path::new(&root_path));
        builder.git_ignore(true);
        builder.add_custom_ignore_filename(".axonignore");

        for result in builder.build() {
            if let Ok(entry) = result {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    let file_path = entry.path().to_string_lossy().into_owned();
                    
                    owned_env.send_and_clear(&pid, |env| {
                        (rustler::types::atom::ok(), file_path).encode(env)
                    });
                }
            }
        }
        
        // Final message to signal completion
        owned_env.send_and_clear(&pid, |env| {
            (rustler::types::atom::ok(), "done").encode(env)
        });
    });

    rustler::types::atom::ok()
}
```

**Step 3: Build the NIF**

Run: `cd src/watcher/native/axon_scanner && cargo build --release`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add src/watcher/native/axon_scanner/src/lib.rs
git commit -m "feat(scanner): add asynchronous streaming support in Rust NIF"
```

---

### Task 2: Update Elixir Scanner Interface

**Files:**
- Modify: `src/watcher/lib/axon/scanner.ex`

**Step 1: Add `start_streaming/2` definition**

```elixir
defmodule Axon.Scanner do
  use Rustler, otp_app: :axon_watcher, crate: "axon_scanner"

  def scan(_path), do: :erlang.nif_error(:nif_not_loaded)
  def start_streaming(_path, _pid), do: :erlang.nif_error(:nif_not_loaded)
end
```

**Step 2: Commit**

```bash
git add src/watcher/lib/axon/scanner.ex
git commit -m "feat(scanner): expose start_streaming to Elixir"
```

---

### Task 3: Refactor Watcher Server for Reactive Streaming

**Files:**
- Modify: `src/watcher/lib/axon/watcher/server.ex`

**Step 1: Update `handle_info(:initial_scan)` to use streaming**

```elixir
  @impl true
  def handle_info(:initial_scan, state) do
    Logger.info("[Pod A] Starting priority streaming scan on: #{state.watch_dir}")
    Axon.Scanner.start_streaming(state.watch_dir, self())
    {:noreply, %{state | pending_files: MapSet.new()}}
  end
```

**Step 2: Add `handle_info` for discovered files with Priority Scoring**

```elixir
  @impl true
  def handle_info({:ok, "done"}, state) do
    Logger.info("[Pod A] Streaming scan completed.")
    Axon.Watcher.Progress.update_status(state.repo_slug, %{status: "live", progress: 100})
    {:noreply, state}
  end

  @impl true
  def handle_info({:ok, path}, state) do
    if should_process?(path) do
      score = calculate_priority(path)
      
      cond do
        score >= 100 ->
          dispatch_batch([path], :indexing_critical)
        true ->
          # Add to pending for batching
          send(self(), :schedule_batch)
          {:noreply, %{state | pending_files: MapSet.put(state.pending_files, path)}}
      end
    end
    {:noreply, state}
  end
```

**Step 3: Implement `calculate_priority/1`**

```elixir
  defp calculate_priority(path) do
    name = Path.basename(path) |> String.downcase()
    ext = Path.extname(path) |> String.downcase()
    
    cond do
      name in ["mix.exs", "cargo.toml", "readme.md", "architecture.md"] -> 100
      Path.dirname(path) == "." and ext in [".ex", ".rs", ".py"] -> 80
      ext in [".ex", ".rs", ".py"] -> 50
      true -> 10
    end
  end
```

**Step 4: Commit**

```bash
git add src/watcher/lib/axon/watcher/server.ex
git commit -m "feat(watcher): implement reactive scoring and streaming orchestration"
```

---

### Task 4: Configure Oban for Priority Queues

**Files:**
- Modify: `src/watcher/config/config.exs` (or appropriate config)

**Step 1: Add `indexing_critical` queue to Oban**

```elixir
config :axon_watcher, Oban,
  queues: [
    indexing_critical: 10,
    indexing_hot: 5,
    indexing_default: 20
  ]
```

**Step 2: Commit**

```bash
git add src/watcher/config/config.exs
git commit -m "config(oban): add priority queues for critical indexing"
```

---

### Task 5: End-to-End Validation

**Step 1: Trigger scan and watch logs**

Run: `mix phx.server` (dans une session interactive)
Action: `Axon.Watcher.Server.trigger_scan()`
Verify: Logs should show "Enqueued batch to indexing_critical" almost immediately.

**Step 2: Check Dashboard Progress**

Verify: Progress bar should advance smoothly instead of jumping from 0 to 100.
