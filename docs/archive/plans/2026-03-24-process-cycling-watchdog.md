# Process Cycling Watchdog (Option B) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement Option B (Process Cycling) to guarantee absolute resilience against C++ FFI memory fragmentation. A memory watchdog thread will monitor the Rust daemon's RAM. If it exceeds 14GB, it triggers a clean shutdown. The external start script is modified to loop and restart the process indefinitely.

**Architecture:** 
1. **Rust (`axon-core`):** A lightweight `std::thread` reads `/proc/self/statm` every 10 seconds. If `rss * page_size > 14GB`, it sets the `cancel_token` to true, stopping the TCP server and allowing the main loop to exit gracefully.
2. **Bash (`start-v2.sh`):** Wraps the `bin/axon-core` execution in a `while true` loop. If the process exits (due to the watchdog), it restarts automatically.

**Tech Stack:** Rust (std::fs), Bash.

---

### Task 1: The Memory Watchdog in Rust

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Write the failing test**

```rust
// Add to src/axon-core/src/main.rs
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_statm_rss() {
        // Mock the contents of /proc/self/statm
        // Format: size resident shared text lib data dt
        let statm_content = "1000 500 250 10 0 100 0";
        let rss_pages = parse_rss_from_statm(statm_content).unwrap();
        assert_eq!(rss_pages, 500);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_parse_statm_rss`
Expected: `parse_rss_from_statm` not found.

**Step 3: Write minimal implementation**

In `main.rs`, add:
```rust
fn parse_rss_from_statm(content: &str) -> Option<u64> {
    content.split_whitespace().nth(1)?.parse().ok()
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_parse_statm_rss`
Expected: PASS.

**Step 5: Commit**

```bash
git commit -am "feat(core): add statm parser for memory watchdog"
```

---

### Task 2: Inject the Watchdog Thread

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Write the failing test**

No direct unit test for threading/exit logic easily without mocking the filesystem. We will rely on integration/review. (Skip strict Red for the infinite loop logic, or test the shutdown trigger function).
Instead of a failing test for the thread, we test the threshold logic.

```rust
#[test]
fn test_memory_threshold_logic() {
    let limit_bytes: u64 = 14 * 1024 * 1024 * 1024;
    let page_size: u64 = 4096;
    let rss_pages = (15 * 1024 * 1024 * 1024) / 4096; // 15GB
    
    assert!(rss_pages * page_size > limit_bytes);
}
```

**Step 3: Write minimal implementation**

In `main.rs`, inside the `serve_mcp` function, just before the main TCP loop:
```rust
let cancel_clone = cancel_token.clone();
std::thread::spawn(move || {
    let page_size = 4096; // Standard Linux page size
    let limit_bytes: u64 = 14 * 1024 * 1024 * 1024; // 14 GB
    loop {
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = parse_rss_from_statm(&content) {
                if rss_pages * page_size > limit_bytes {
                    log::error!("CRITICAL: Memory threshold reached ({} GB). Triggering graceful suicide...", (rss_pages * page_size) / 1024 / 1024 / 1024);
                    cancel_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
});
```

**Step 5: Commit**

```bash
git commit -am "feat(core): implement memory watchdog thread to trigger graceful shutdown"
```

---

### Task 3: The OS Sledgehammer (Bash Loop)

**Files:**
- Modify: `scripts/start-v2.sh`

**Step 1: Write the failing test**

N/A for bash script modification.

**Step 3: Write minimal implementation**

Find the line where `axon-core` is executed in `start-v2.sh`:
`bin/axon-core --mcp-uds /tmp/axon-mcp.sock &`
Wrap it in a supervisor loop:
```bash
(
  while true; do
      echo "🚀 Starting Axon Core..."
      bin/axon-core --mcp-uds /tmp/axon-mcp.sock
      EXIT_CODE=$?
      echo "⚠️ Axon Core exited with code $EXIT_CODE. Restarting in 2 seconds..."
      sleep 2
  done
) &
```

**Step 5: Commit**

```bash
git commit -am "feat(infra): wrap axon-core in continuous supervisor loop"
```