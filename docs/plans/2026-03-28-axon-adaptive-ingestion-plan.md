# Axon Ingestion - Adaptive Traffic Guardian Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the "Nexus Pull" sliding window ingestion. Elixir will adaptively pull pending files from the Rust Data Plane based on real-time DuckDB commit latencies (T4).

**Architecture:**
1. **Rust Side:** Add `PULL_PENDING` command to fetch prioritized files from `ist.db`.
2. **Elixir Side:** Create `TrafficGuardian` to monitor `Tracer` metrics and regulate the flow by sending `PULL_PENDING` requests.
3. **Loopback:** Ensure indexed files are marked as such in the DB to prevent re-pulling.

**Tech Stack:** Rust, Elixir/OTP, DuckDB (SQL), Unix Domain Sockets.

---

### Task 1: Rust - Fetch Pending Files Logic

**Files:**
- Modify: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/main.rs`

**Step 1: Write the failing test**
In `graph.rs`, add a test to verify we can fetch a batch of pending files.
```rust
    #[test]
    fn test_fetch_pending_batch() {
        let store = GraphStore::new(":memory:").unwrap();
        store.execute("INSERT INTO File (path, status, priority) VALUES ('f1.rs', 'pending', 100), ('f2.rs', 'indexed', 50)").unwrap();
        let res = store.fetch_pending_batch(10).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].path, "f1.rs");
    }
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test test_fetch_pending_batch`
Expected: FAIL (method not found)

**Step 3: Write minimal implementation**
1. Add `pub fn fetch_pending_batch(&self, count: usize) -> Result<Vec<PendingFile>>` to `GraphStore`.
2. Update `main.rs` socket loop to handle `PULL_PENDING <count>` and respond with a JSON message.

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test test_fetch_pending_batch`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/graph.rs src/axon-core/src/main.rs
git commit -m "feat(core): implement PULL_PENDING command for adaptive ingestion"
```

---

### Task 2: Elixir - The Traffic Guardian Engine

**Files:**
- Create: `src/dashboard/lib/axon_nexus/axon/watcher/traffic_guardian.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/application.ex`

**Step 1: Write the failing test**
Create `src/dashboard/test/axon_watcher/traffic_guardian_test.exs` to verify pressure adjustment logic.

**Step 2: Run test to verify it fails**
Run: `cd src/dashboard && mix test test/axon_watcher/traffic_guardian_test.exs`
Expected: FAIL

**Step 3: Write minimal implementation**
1. Implement the `TrafficGuardian` GenServer.
2. Subscribe to `:telemetry` events for T4 metrics.
3. Periodically check the ETS hopper and call `PoolFacade.pull_pending/1`.
4. Integrate into the supervision tree.

**Step 4: Run test to verify it passes**
Run: `cd src/dashboard && mix test`
Expected: PASS

**Step 5: Commit**
```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/traffic_guardian.ex
git commit -m "feat(watcher): implement adaptive Traffic Guardian for Nexus Pull ingestion"
```

---

### Task 3: Dashboard - Real-time Backpressure Visualization

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard_web/live/cockpit_live.ex`
- Modify: `src/dashboard/lib/axon_dashboard_web/live/cockpit_live.html.heex`

**Step 1: Write the failing test**
Verify that the `target_pressure` metric is present in the assigns.

**Step 2: Run test to verify it fails**
Expected: FAIL

**Step 3: Write minimal implementation**
1. Expose `target_pressure` and `t4_ema` from the Guardian to the UI via PubSub.
2. Add a visual "Pressure Gauge" or a line chart showing the backpressure regulation.

**Step 4: Run test to verify it passes**
Expected: PASS

**Step 5: Commit**
```bash
git add src/dashboard/lib/axon_dashboard_web/live/
git commit -m "feat(ui): add real-time backpressure monitoring to the cockpit"
```
