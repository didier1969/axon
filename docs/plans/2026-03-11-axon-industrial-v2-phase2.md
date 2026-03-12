# Axon v2 : Le Pont & Le Dashboard (Phase 2) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Establish low-latency communication between the Rust Data Plane and an Elixir-based Dashboard.

**Architecture:** Data Plane (Rust) acts as a UDS Server streaming MsgPack events. Control Plane (Elixir/Phoenix) acts as a UDS Client, consuming events and displaying them via LiveView.

**Tech Stack:** Rust (Tokio, MsgPack), Elixir 1.18, Phoenix 1.7, Unix Domain Sockets.

---

### Task 5: Implement UDS Server (Rust)

**Files:**
- Modify: `src/axon-core/Cargo.toml`
- Create: `src/axon-core/src/bridge.rs`
- Modify: `src/axon-core/src/main.rs`

**Step 1: Add dependencies**
```toml
tokio = { version = "1.36", features = ["full", "net"] }
rmp-serde = "1.1"
```

**Step 2: Define Message Protocol (bridge.rs)**
```rust
#[derive(Serialize, Deserialize, Debug)]
pub enum BridgeEvent {
    FileIndexed { path: String, symbol_count: usize },
    ScanComplete { total_files: usize, duration_ms: u64 },
}
```

**Step 3: Implement UDS Server**
- Bind to `/tmp/axon-v2.sock`.
- Handle multi-client (though only one dashboard usually).
- Stream events during the parallel processing loop.

**Step 4: Commit**
```bash
git add src/axon-core/
git commit -m "feat: implement UDS server with MsgPack streaming"
```

---

### Task 6: Initialize Elixir Dashboard (Control Plane)

**Files:**
- Create: `src/dashboard/` (New Phoenix project)

**Step 1: Generate Phoenix project**
Run: `mix phx.new src/dashboard --no-ecto --no-mailer --no-gettext --no-dashboard`

**Step 2: Add dependencies (mix.exs)**
```elixir
{:msgpax, "~> 2.3"},
```

**Step 3: Commit**
```bash
git add src/dashboard/
git commit -m "infra: initialize phoenix dashboard for axon v2"
```

---

### Task 4: UDS Client & LiveView (Elixir)

**Files:**
- Create: `src/dashboard/lib/axon_dashboard/bridge_client.ex`
- Create: `src/dashboard/lib/axon_dashboard_web/live/status_live.ex`

**Step 1: Implement BridgeClient (GenServer)**
- Connect to `/tmp/axon-v2.sock`.
- Decode MsgPack.
- Broadcast to Phoenix.PubSub.

**Step 2: Create Status LiveView**
- Subscribe to PubSub.
- Display a real-time list of "Last Indexed Files".
- Display a progress bar/stats.

**Step 3: Validation**
- Launch Rust Data Plane.
- Launch Phoenix Dashboard.
- Verify that files appearing in Rust terminal also appear in the Browser.

**Step 4: Commit**
```bash
git commit -m "feat: connect elixir dashboard to rust core via UDS"
```
