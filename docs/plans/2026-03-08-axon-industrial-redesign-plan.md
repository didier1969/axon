# Axon Industrial Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform Axon into a robust, 100% reliable code intelligence engine with a Rust-powered scanner, unified configuration, and HydraDB-based reporting.

**Architecture:** Triple-Pod architecture where Pod A (Watcher) uses a Rust NIF for file discovery and OS-native surveillance. Status reporting is centralized in HydraDB v1.0.0. The engine is exposed as a persistent MCP server.

**Tech Stack:** Elixir 1.18, Rust (Rustler, ignore, notify), Python 3.12, HydraDB v1.0.0, MCP Protocol.

---

### Task 1: Setup Rust Infrastructure (NIF)

**Files:**
- Create: `src/watcher/native/axon_scanner/.cargo/config.toml`
- Create: `src/watcher/native/axon_scanner/Cargo.toml`
- Create: `src/watcher/native/axon_scanner/src/lib.rs`
- Modify: `src/watcher/mix.exs`

**Step 1: Add Rustler to mix.exs**
```elixir
defp deps do
  [
    {:rustler, "~> 0.30.0"},
    # ... existing deps
  ]
end
```

**Step 2: Initialize Rustler project**
Run: `cd src/watcher && mix rustler.init --name axon_scanner`
Expected: Folder `native/axon_scanner` created.

**Step 3: Define Cargo.toml with dependencies**
```toml
[dependencies]
rustler = "0.30.0"
ignore = "0.4"
notify = "6.1"
sha2 = "0.10"
```

**Step 4: Commit setup**
```bash
git add src/watcher/mix.exs src/watcher/native/axon_scanner/
git commit -m "infra: setup rustler and cargo for axon_scanner"
```

---

### Task 2: Implement High-Performance Scan (Rust)

**Files:**
- Modify: `src/watcher/native/axon_scanner/src/lib.rs`
- Create: `src/watcher/lib/axon/scanner.ex`

**Step 1: Write Rust Scan function (lib.rs)**
```rust
#[rustler::nif]
fn scan(path: String) -> Vec<String> {
    use ignore::WalkBuilder;
    let mut files = Vec::new();
    for result in WalkBuilder::new(path).build() {
        if let Ok(entry) = result {
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                files.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    files
}
```

**Step 2: Write Elixir Bridge (scanner.ex)**
```elixir
defmodule Axon.Scanner do
  use Rustler, otp_app: :axon_watcher, crate: "axon_scanner"
  def scan(_path), do: :erlang.nif_error(:nif_not_loaded)
end
```

**Step 3: Write test in Elixir**
```elixir
defmodule Axon.ScannerTest do
  use ExUnit.Case
  test "scans a directory" do
    files = Axon.Scanner.scan(".")
    assert length(files) > 0
  end
end
```

**Step 4: Run test**
Run: `cd src/watcher && mix test tests/axon/scanner_test.exs`
Expected: PASS

---

### Task 3: Implement Unified .axonignore Cascade

**Files:**
- Modify: `src/watcher/native/axon_scanner/src/lib.rs`
- Create: `.axonignore` (Global)

**Step 1: Implement cascade logic in Rust**
- Load patterns from `/home/dstadel/projects/.axonignore`
- Load patterns from `axon/.axonignore`
- Load patterns from project-local `.axonignore`
- Apply `.md` force-include rule.

**Step 2: Write test cases for ignore rules**
- Create dummy folder with `.axonignore`
- Verify Rust scanner respects hierarchy and overrides.

**Step 3: Commit**
```bash
git commit -m "feat: implement unified .axonignore hierarchy in rust"
```

---

### Task 4: Centralize Truth in HydraDB

**Files:**
- Modify: `src/watcher/lib/axon/watcher/progress.ex`
- Modify: `src/axon/cli/main.py`

**Step 1: Remove status.json calls**
- Delete `write_local_status` in `progress.ex`.

**Step 2: Implement HydraDB bulk metadata fetch in Python**
```python
def get_fleet_status(self):
    # One query to get all axon:repo:* keys
    return self.execute_raw("MATCH (n:Metadata) WHERE n.key STARTS WITH 'axon:repo:' RETURN n")
```

**Step 3: Update CLI to use HydraDB data only**
- Remove file reading in `fleet_status`.
- Add columns for `last_scan_at` and `last_file_import_at`.

---

### Task 5: Always-On Native Surveillance

**Files:**
- Modify: `src/watcher/native/axon_scanner/src/lib.rs`
- Create: `scripts/axon-service-install.sh`

**Step 1: Implement Rust Notify loop**
- Watch for changes using native OS API.
- Send events to Elixir via `Rustler::Job`.

**Step 2: Systemd/Windows Service Script**
- Create Unit file template.
- Implement `axon start/stop/restart` in CLI.

---

### Task 6: MCP Server Exposure

**Files:**
- Modify: `src/axon/mcp/server.py`
- Update: `GEMINI.md`, `CLAUDE.md`

**Step 1: Register all tools in MCP server**
- `axon_query`, `axon_summarize`, `axon_fleet_status`.

**Step 2: Update documentation**
- Document every MCP tool with examples.

**Step 3: Final E2E Validation**
- Launch global service.
- Query via Gemini CLI to verify data from HydraDB.
