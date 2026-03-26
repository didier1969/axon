# Axon Native Backpressure & Yielding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implémenter l'architecture V2.1 "Mission-Critical". Éradiquer la file d'attente SQLite de Rust (pour la remplacer par un canal mémoire borné) et forcer le Writer Actor à "céder le passage" (Yield) aux requêtes IA, tout en renvoyant un feedback clair (Busy/Poison) à Oban pour gérer les retrys nativement.

**Architecture:** La base de données SQLite locale `tasks.db` de Rust sera supprimée. Le scanner rust poussera directement dans un `tokio::sync::mpsc::channel`. Le `Writer Actor` utilisera `try_write_for(100ms)` : s'il échoue, il émettra un événement UDS `status: "busy"`. S'il panique sémantiquement, un `status: "error"`. L'application Elixir gérera ces retours pour appliquer son Exponential Backoff ou marquer le fichier en `POISON`.

**Tech Stack:** Rust (Tokio MPSC, RwLock), Elixir (Oban, Ecto).

---

### Task 1: Démantèlement de la base SQLite Rust (queue.rs)

**Files:**
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/worker.rs`

**Step 1: Write the failing test**
(Note: Le TDD pour ce cas spécifique consiste à vérifier que le système compile avec le nouveau type MPSC, et que les vieux tests SQLite échouent car le module est redéfini).

**Step 2: Rewrite queue.rs to pure RAM struct**
Remplacer la structure `QueueStore` qui wrappe `rusqlite` par un wrapper autour de `crossbeam_channel::bounded` (ou `tokio::mpsc` si on passe tout en async, mais `crossbeam` est excellent pour le multi-threading CPU des workers actuels).

```rust
// In src/axon-core/src/queue.rs
use crossbeam_channel::{bounded, Sender, Receiver};
use tracing::{info, error, info_span};

#[derive(Debug, Clone)]
pub struct Task {
    pub path: String,
    pub trace_id: String,
    pub t0: i64,
    pub t1: i64,
    pub t2: i64,
}

pub struct QueueStore {
    sender: Sender<Task>,
    receiver: Receiver<Task>,
}

impl QueueStore {
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = bounded(capacity);
        Self { sender, receiver }
    }

    pub fn push(&self, path: &str, _mtime: i64, trace_id: &str, t0: i64, t1: i64) -> Result<(), String> {
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let task = Task {
            path: path.to_string(),
            trace_id: trace_id.to_string(),
            t0, t1, t2
        };
        self.sender.send(task).map_err(|e| format!("Channel full or dead: {}", e))
    }

    pub fn pop(&self) -> Option<Task> {
        self.receiver.recv().ok()
    }

    pub fn try_pop(&self) -> Option<Task> {
        self.receiver.try_recv().ok()
    }

    // This is a no-op now, state is managed by Elixir
    pub fn mark_done(&self, _path: &str) {}
    
    // Clear the channel
    pub fn purge_all(&self) {
        while self.receiver.try_recv().is_ok() {}
    }
}
```

**Step 3: Modify main.rs and scanner to drop SQLite logic**
Remove SQLite `tasks.db` path definitions in `main.rs` and update `QueueStore::new(500)`.

**Step 4: Run tests and confirm compilation**
Run: `cargo check --manifest-path src/axon-core/Cargo.toml`
Expected: Passes.

**Step 5: Commit**
```bash
git add src/axon-core/src/
git commit -m "refactor(core): replace SQLite queue with bounded in-memory channel for native backpressure"
```

---

### Task 2: Refonte du Writer Actor (Traffic Shaping & Yielding)

**Files:**
- Modify: `src/axon-core/src/worker.rs`
- Modify: `src/axon-core/src/bridge.rs`

**Step 1: Extend BridgeEvent to support Rejection (Feedback)**

Dans `src/axon-core/src/bridge.rs`, modifier l'enum `BridgeEvent` pour ajouter `status` et `error_reason`.

```rust
    FileIndexed { 
        path: String, 
        status: String, // "ok", "busy", "error"
        error_reason: String,
        symbol_count: usize,
        relation_count: usize,
        file_count: usize,
        entry_points: usize,
        security_score: usize,
        coverage_score: usize,
        #[serde(default)]
        taint_paths: String,
        trace_id: String,
        t0: i64, t1: i64, t2: i64, t3: i64, t4: i64,
    },
```

**Step 2: Update Writer Actor to use `try_write_for` and return exact feedback**

Dans `src/axon-core/src/worker.rs`:
```rust
                    Ok(task) => {
                        let _span = tracing::info_span!("db_writer_task", path = %task.path).entered();
                        let symbols_count = task.extraction.symbols.len();
                        let relations_count = task.extraction.relations.len();

                        let mut status = "ok".to_string();
                        let mut error_reason = "".to_string();

                        // THE TOC YIELD: If an Agent is reading, we yield instantly.
                        if mcp_active.load(Ordering::Relaxed) > 0 {
                            status = "busy".to_string();
                            error_reason = "System Contention (Agent Reading)".to_string();
                        } else {
                            // Try to get write lock for 100ms. If we can't, it's a contention.
                            match graph_store.try_write_for(std::time::Duration::from_millis(100)) {
                                Some(mut store) => {
                                    if let Err(e) = store.insert_file_data(&task.path, &task.extraction) {
                                        error!("Writer Actor failed to insert {}: {:?}", task.path, e);
                                        status = "error".to_string();
                                        error_reason = format!("{:?}", e);
                                    }
                                },
                                None => {
                                    error!("Writer Actor timeout (100ms) waiting for KuzuDB lock on {}", task.path);
                                    status = "busy".to_string();
                                    error_reason = "KuzuDB Write Lock Timeout".to_string();
                                }
                            }
                        }
                        
                        let t4 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
                        
                        let finish_msg = match serde_json::to_string(&crate::bridge::BridgeEvent::FileIndexed {
                            path: task.path.clone(),
                            status,
                            error_reason,
                            symbol_count: symbols_count,
                            relation_count: relations_count,
                            file_count: 1,
                            entry_points: 0,
                            security_score: 100,
                            coverage_score: 0,
                            taint_paths: "".to_string(),
                            trace_id: task.trace_id,
                            t0: task.t0, t1: task.t1, t2: task.t2, t3: task.t3, t4,
                        }) {
                            Ok(msg) => msg + "\n",
                            Err(_) => continue,
                        };
                        let _ = result_sender.send(finish_msg);
                    },
```

**Step 3: Run Rust tests**
Fix any compilation errors due to `BridgeEvent` changes in `main.rs` or `worker.rs`.

**Step 4: Commit**
```bash
git commit -am "feat(core): implement Writer Actor yielding and informative rejection routing"
```

---

### Task 3: Câblage du "Smart Retry" et "Poison" dans Elixir (Oban)

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/indexing_worker.ex`

**Step 1: Update PoolFacade to parse the new status field**
Dans `src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex`:

```elixir
        {:ok, %{"FileIndexed" => payload}} ->
          path = payload["path"]
          status = payload["status"] || "ok"
          error_reason = payload["error_reason"] || ""
          
          # ... (keep telemetry extraction) ...
          
          # Update the calling process with the exact status
          case state.requests[path] do
            from when not is_nil(from) ->
              GenServer.reply(from, %{"status" => status, "error_reason" => error_reason})
            _ -> :ok
          end
```

**Step 2: Update IndexingWorker to implement the TOC Router**
Dans `src/dashboard/lib/axon_nexus/axon/watcher/indexing_worker.ex`:

```elixir
            case PoolFacade.parse(path, lane, trace_id, t0, t1) do
              %{"status" => "ok"} ->
                # (Keep existing success logic, mark_file_status! "indexed")
                
              %{"status" => "busy", "error_reason" => reason} ->
                Logger.warning("[Oban] Contention/Busy on #{path}: #{reason}. Backing off.")
                Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, reason})
                # By returning an error tuple, Oban natively triggers Exponential Backoff
                raise "System Contention: #{reason}"

              %{"status" => "error", "error_reason" => reason} ->
                Logger.error("[Oban] Fatal parsing error on #{path}: #{reason}. Marking POISON.")
                Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, reason})
                
                # Mark as Poison and Do Not Retry
                try do
                  Axon.Watcher.Tracking.mark_file_status!(path, "poison", %{error_reason: reason})
                rescue
                  _ -> :ok
                end
                # We return :ok so Oban discards the job (we don't want to retry a poison file)
            end
```

**Step 3: Run Elixir compilation**
Run: `cd src/dashboard && mix compile`

**Step 4: Commit**
```bash
git commit -am "feat(nexus): route rust rejection to Oban exponential backoff and poison tracker"
```
