# Adaptive Priority Queue (Lazy vs Eager) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implémenter une stratégie de file d'attente Oban progressive (Lazy/Hot Path vs Eager/Cold Path) pour prioriser l'indexation réactive de fichiers sur le scan massif au démarrage, accélérant ainsi la boucle de feedback.

**Architecture:** Mettre à jour la configuration d'Oban dans Elixir pour supporter deux files (`hot: 2` et `default: 1`). L'initialisation massive d'Axon poussera les requêtes dans `default` (priorité basse). Les requêtes déclenchées par l'outil de surveillance de fichiers (modification de code) pousseront le dossier parent ciblé dans la file `hot` (priorité haute).

**Tech Stack:** Elixir, Oban.

---

### Task 1: Update Oban Queue Configuration

**Files:**
- Modify: `src/watcher/config/config.exs`

**Step 1: Write the failing test**
_Pas de test unitaire pour la configuration globale, nous allons directement modifier le fichier._

**Step 2: Write minimal implementation**
Localiser la configuration `config :axon_watcher, Oban,` et remplacer :
```elixir
  queues: [indexing: 10]
```
Par :
```elixir
  queues: [indexing_hot: [limit: 5], indexing_default: [limit: 10]]
```
_Note: On utilisera deux files distinctes pour simplifier le routage prioritaire._

**Step 3: Commit**
```bash
git add src/watcher/config/config.exs
git commit -m "chore(watcher): configure Oban queues for hot and default paths"
```

---

### Task 2: Adapt IndexingWorker to support Queue Options

**Files:**
- Modify: `src/watcher/lib/axon/watcher/indexing_worker.ex`

**Step 1: Write minimal implementation**
Retirer la mention statique de la queue pour permettre l'insertion dynamique via les options.

Modifier :
```elixir
  use Oban.Worker, queue: :indexing, max_attempts: 3
```
En :
```elixir
  use Oban.Worker, queue: :indexing_default, max_attempts: 3
```
(On garde `indexing_default` comme fallback implicite)

**Step 2: Commit**
```bash
git add src/watcher/lib/axon/watcher/indexing_worker.ex
git commit -m "refactor(worker): update Oban worker to support dynamic queue targeting"
```

---

### Task 3: Route Initial Scan to Default Queue (Cold Path)

**Files:**
- Modify: `src/watcher/lib/axon/watcher/server.ex`

**Step 1: Write minimal implementation**
Dans `server.ex`, localiser la fonction `dispatch_batch(paths)` et lui ajouter un paramètre d'options (ex. la queue ciblée).

Créer deux versions de `dispatch_batch` ou modifier la signature existante pour accepter le nom de la file.

```elixir
  defp dispatch_batch(paths, queue \\ :indexing_default) do
    files_payload = Enum.reduce(paths, [], fn path, acc -> ... end)

    if length(files_payload) > 0 do
      try do
        job_args = %{"batch" => files_payload}
        Axon.Watcher.IndexingWorker.new(job_args, queue: queue)
        |> Oban.insert!()
        Logger.info("[Pod A] Enqueued batch of #{length(files_payload)} files to #{queue}.")
      rescue
        e -> Logger.error("[Pod A] FAILED to enqueue batch: #{inspect(e)}")
      end
    end
  end
```

Modifier la ligne dans `handle_info(:initial_scan)`:
```elixir
  files |> Enum.chunk_every(@max_batch_size) |> Enum.each(&dispatch_batch(&1, :indexing_default))
```

**Step 2: Commit**
```bash
git add src/watcher/lib/axon/watcher/server.ex
git commit -m "feat(server): route initial scan batches to default background queue"
```

---

### Task 4: Route File Events to Hot Queue (Hot Path)

**Files:**
- Modify: `src/watcher/lib/axon/watcher/server.ex`

**Step 1: Write minimal implementation**
Dans `server.ex`, modifier `handle_info(:process_batch)` pour pousser les requêtes modifiées de façon prioritaire.

```elixir
  @impl true
  def handle_info(:process_batch, state) do
    files_to_process = MapSet.to_list(state.pending_files)
    if length(files_to_process) > 0 do
      files_to_process 
      |> Enum.chunk_every(@max_batch_size) 
      |> Enum.each(&dispatch_batch(&1, :indexing_hot))
    end
    {:noreply, %{state | pending_files: MapSet.new(), timer: nil}}
  end
```

**Step 2: Commit**
```bash
git add src/watcher/lib/axon/watcher/server.ex
git commit -m "feat(server): route file modification events to hot priority queue"
```

---

### Task 5: Enhance Hot Path with Active Directory Clustering

**Files:**
- Modify: `src/watcher/lib/axon/watcher/server.ex`

**Step 1: Write minimal implementation**
Dans `handle_info({:file_event})`, lorsqu'un fichier est modifié, ajouter non seulement ce fichier, mais scanner également son dossier parent direct pour l'ajouter à `pending_files` (Clustering de proximité).

```elixir
  @impl true
  def handle_info({:file_event, _pid, {path, events}}, state) do
    str_path = to_string(path)
    if state.monitoring_active and should_process?(str_path) do
      if :deleted in events do
        {:noreply, state}
      else
        parent_dir = Path.dirname(str_path)
        
        # Obtenir les fichiers voisins (proximité architecturale)
        neighbors = 
          case File.ls(parent_dir) do
            {:ok, files} -> 
              Enum.map(files, &Path.join(parent_dir, &1))
              |> Enum.filter(&should_process?/1)
              |> Enum.filter(&(File.regular?(&1)))
            _ -> []
          end
        
        # Fusionner avec les fichiers en attente
        new_pending = Enum.reduce([str_path | neighbors], state.pending_files, &MapSet.put(&2, &1))
        
        new_timer = reset_timer(state.timer)
        {:noreply, %{state | pending_files: new_pending, timer: new_timer}}
      end
    else
      {:noreply, state}
    end
  end
```

**Step 2: Commit**
```bash
git add src/watcher/lib/axon/watcher/server.ex
git commit -m "feat(server): implement directory clustering for hot path queueing"
```

---

### Task 6: Roadmap Update

**Files:**
- Modify: `ROADMAP.md`

**Step 1: Write minimal implementation**
Marquer la stratégie "Lazy vs Eager" comme terminée.

```markdown
- [x] **Stratégie Lazy vs Eager :** Implémentation de la file d'attente de tâches de fond via OTP (supervision Elixir).
```

**Step 2: Commit**
```bash
git add ROADMAP.md
git commit -m "docs: mark Adaptive Priority Queue phase as complete"
```