# Axon Writer Actor Micro-Batching Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implémenter le "Smart Drain" et le "Micro-Batching avec Fallback" dans le Writer Actor (Rust) pour démultiplier le débit d'insertion KuzuDB tout en isolant les "Poison Pills".

**Architecture:** Le `Writer Actor` (`src/axon-core/src/worker.rs`) va passer d'une boucle unitaire à une boucle par lots (batch). Il attendra le premier message avec `recv()`, puis videra opportunistement le canal mémoire jusqu'à 50 tâches (`try_recv()`). Ensuite, il tentera d'insérer ces 50 fichiers en une seule transaction `store.execute_batch` (méthode native ajoutée à `graph.rs`). Si cette transaction globale échoue (Poison Pill dans le lot), le système basculera en mode *Slow Path* et réessaiera le lot fichier par fichier pour isoler l'erreur, garantissant la Tolérance Zéro (pas de perte de données saines).

**Tech Stack:** Rust (KuzuDB API, Crossbeam Channels).

---

### Task 1: Refactoring du `GraphStore` (Rust) pour supporter les transactions par lots

**Files:**
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Write the failing test**
(Le TDD vérifiera que la nouvelle méthode `insert_file_data_batch` est appelable et fonctionne sur un lot de `ExtractionResult`).

**Step 2: Add `insert_file_data_batch` method to `GraphStore`**

Dans `src/axon-core/src/graph.rs`, au lieu de juste faire un `execute_param` pour *chaque* requête d'un fichier (ce qui auto-commit), nous allons utiliser la sémantique de transaction explicite de Ladybug/KuzuDB si elle est disponible, ou simuler un batch efficace via la construction d'une grosse chaîne de requêtes (ou idéalement exécuter séquentiellement en gardant le `store` verrouillé, ce qui est déjà le cas puisque l'acteur prend le `write()`).
*Remarque de l'architecte : KuzuDB (via notre wrapper C-FFI) n'expose pas de méthode `BEGIN TRANSACTION` simple en C-FFI pour l'instant. Le simple fait de regrouper les appels d'exécution tout en conservant le `RwLockWriteGuard` amortira le Lock Contention global du cluster, mais c'est surtout la réduction des appels UDS/Elixir et la minimisation des timeouts d'acquisition du Lock qui feront la différence.*

Pour rester "Safe" par rapport au driver C-FFI actuel, nous allons garder `insert_file_data` tel quel pour la mécanique de base (qui fait les `execute_param`), mais l'Actor va boucler *à l'intérieur* de son acquisition de Lock au lieu de le relâcher à chaque fichier.

*(Aucune modification de `graph.rs` n'est en fait strictement nécessaire si on boucle dans l'Actor tout en tenant le WriteGuard. On passe à la Task 2 pour la logique métier).*

---

### Task 2: Implémentation du Smart Drain (Micro-Batching) dans le Writer Actor

**Files:**
- Modify: `src/axon-core/src/worker.rs`

**Step 1: Write the failing test**
(Vérifier que le worker compile avec la nouvelle logique de vecteur `Vec<DbWriteTask>`).

**Step 2: Rewrite `spawn_writer_actor` to use batching**

Dans `src/axon-core/src/worker.rs`, remplacer la boucle principale du Writer Actor :

```rust
    fn spawn_writer_actor(
        receiver: Receiver<DbWriteTask>,
        graph_store: Arc<RwLock<GraphStore>>,
        mcp_active: Arc<AtomicUsize>,
        queue: Arc<QueueStore>,
        result_sender: tokio::sync::broadcast::Sender<String>
    ) {
        thread::Builder::new().name("axon-db-writer".to_string()).spawn(move || {
            info!("Writer Actor born. Holding exclusive keys to KuzuDB.");
            
            const BATCH_SIZE: usize = 50;
            let mut batch = Vec::with_capacity(BATCH_SIZE);

            loop {
                // Yield immediately if MCP is querying to ensure 0 latency
                while mcp_active.load(Ordering::Acquire) > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }

                // 1. BLOCKING WAIT for the first task (No CPU spin)
                let first_task = match receiver.recv() {
                    Ok(t) => t,
                    Err(_) => break, // Channel disconnected
                };
                
                batch.push(first_task);

                // 2. SMART DRAIN: Opportunistically pull up to BATCH_SIZE
                while batch.len() < BATCH_SIZE {
                    match receiver.try_recv() {
                        Ok(t) => batch.push(t),
                        Err(crossbeam_channel::TryRecvError::Empty) => break, // Channel empty, flush now!
                        Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                    }
                }

                // 3. EXECUTION PHASE (With Yield Protection)
                if mcp_active.load(Ordering::Acquire) > 0 {
                    // LLM active, reject entire batch with "busy"
                    for task in batch.drain(..) {
                        Self::send_feedback(&result_sender, &task, "busy", "System Contention (Agent Reading)");
                    }
                    continue;
                }

                // Try to acquire write lock
                match graph_store.try_write() {
                    Some(store) => {
                        // FAST PATH: Process the entire batch while holding the lock ONCE
                        for task in batch.drain(..) {
                            let _span = tracing::info_span!("db_writer_task", path = %task.path).entered();
                            if let Err(e) = store.insert_file_data(&task.path, &task.extraction) {
                                error!("Writer Actor failed to insert {}: {:?}", task.path, e);
                                Self::send_feedback(&result_sender, &task, "error", &format!("{:?}", e));
                            } else {
                                let _ = queue.mark_done(&task.path);
                                Self::send_feedback(&result_sender, &task, "ok", "");
                            }
                        }
                    },
                    None => {
                        // LOCK STARVATION: Reject entire batch
                        tracing::warn!("Writer Actor KuzuDB lock busy, yielding {} tasks", batch.len());
                        for task in batch.drain(..) {
                            Self::send_feedback(&result_sender, &task, "busy", "KuzuDB Write Lock Contention");
                        }
                    }
                }
            }
        }).expect("Failed to spawn Writer Actor");
    }

    // Helper fonction for telemetry
    fn send_feedback(
        result_sender: &tokio::sync::broadcast::Sender<String>, 
        task: &DbWriteTask, 
        status: &str, 
        error_reason: &str
    ) {
        let t4 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let msg = match serde_json::to_string(&crate::bridge::BridgeEvent::FileIndexed {
            path: task.path.clone(),
            status: status.to_string(),
            error_reason: error_reason.to_string(),
            symbol_count: task.extraction.symbols.len(),
            relation_count: task.extraction.relations.len(),
            file_count: 1,
            entry_points: 0,
            security_score: 100,
            coverage_score: 0,
            taint_paths: "".to_string(),
            trace_id: task.trace_id.clone(),
            t0: task.t0, t1: task.t1, t2: task.t2, t3: task.t3, t4,
        }) {
            Ok(m) => m + "\n",
            Err(_) => return,
        };
        let _ = result_sender.send(msg);
    }
```

**Step 3: Run Rust tests and check for syntax errors**
Run: `cargo test --manifest-path src/axon-core/Cargo.toml`
Expected: Passes perfectly. Le comportement externe (UDS) est identique, mais le débit interne sera de `N` insertions pour 1 acquisition de Lock.

**Step 4: Commit**
```bash
git add src/axon-core/src/
git commit -m "feat(core): implement Actor smart drain and batch processing to optimize KuzuDB I/O limits"
```
