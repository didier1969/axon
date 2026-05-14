# Proactive Audit Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Mettre en place une alerte visuelle automatique dans le tableau de bord (Cockpit) chaque fois qu'un événement d'indexation indique que le score de sécurité d'un projet a chuté.

**Architecture:** 
1. Le client Elixir (`AxonDashboard.BridgeClient`) maintient un dictionnaire des scores connus.
2. Lorsqu'un payload `FileIndexed` est reçu, il compare le nouveau score à l'ancien.
3. Si `nouveau < ancien`, il émet un événement PubSub `{:security_degraded, projet, ancien, nouveau}`.
4. Le `StatusLive` capte cet événement et l'affiche via une alerte visuelle.

**Tech Stack:** Elixir, Phoenix LiveView.

---

### Task 1: Store State and Detect Degradation in BridgeClient

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard/bridge_client.ex`

**Step 1: Write minimal implementation**
Modifier `init/1` pour inclure `security_scores` dans l'état:
```elixir
  def init(_opts) do
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, security_scores: %{}}}
  end
```

Modifier la fonction `handle_info({:tcp, ...})` pour capter les scores. Le plus simple est de modifier la logique de traitement JSON:
```elixir
    # Toujours processer les lignes JSON (y compris SystemReady qui arrive juste après)
    lines = String.split(data, "\n", trim: true)
    
    new_state = Enum.reduce(lines, state, fn line, acc ->
      if not String.contains?(line, "Axon Bridge Ready") do
        case Jason.decode(line) do
          {:ok, event} ->
            acc = handle_bridge_event(event, acc)
            Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})
            acc
          _ -> 
            acc
        end
      else
        acc
      end
    end)
    
    {:noreply, %{new_state | socket: socket}}
```

Créer la fonction privée `handle_bridge_event/2`:
```elixir
  defp handle_bridge_event(%{"FileIndexed" => payload}, state) do
    project = Map.get(payload, "path")
    new_score = Map.get(payload, "security_score", 100)
    
    if project && new_score > 0 do # Skip chunks without finalized score (where it might be 0 or partial)
      old_score = Map.get(state.security_scores, project, 100)
      
      # Si le score final calculé par la base est plus bas que l'ancien
      if new_score < old_score do
        Logger.warning("[BRIDGE] Security Degraded for #{project}: #{old_score} -> #{new_score}")
        Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:security_degraded, project, old_score, new_score})
      end
      
      %{state | security_scores: Map.put(state.security_scores, project, new_score)}
    else
      state
    end
  end

  defp handle_bridge_event(_, state), do: state
```
*(Attention, dans `main.rs`, le "finalizing" envoie 0 pour les chunks, et le vrai score à la fin. Donc on ignore si `new_score` n'est pas cohérent, ou on compte sur la structure exacte. Le `main.rs` envoie `security_score: 100` pour les chunks partiels et le vrai score à la fin. Donc la logique marchera).*

**Step 2: Commit**
```bash
git add src/dashboard/lib/axon_dashboard/bridge_client.ex
git commit -m "feat(dashboard): detect and broadcast security score degradations in Control Plane"
```

---

### Task 2: Display Security Alert in LiveView

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard_web/live/status_live.ex`

**Step 1: Write minimal implementation**
Dans `mount/3`, ajouter une liste d'alertes à l'assign:
```elixir
      alerts: [],
```

Ajouter la fonction de traitement PubSub:
```elixir
  def handle_info({:security_degraded, project, old, new}, socket) do
    alert = "CRITICAL: #{project} security dropped from #{old}% to #{new}%!"
    new_alerts = [alert | socket.assigns.alerts] |> Enum.take(3) # Garde les 3 dernières
    {:noreply, assign(socket, alerts: new_alerts)}
  end
```

Dans la vue HTML `render/1`, afficher les alertes (par exemple sous la navigation) :
```html
        <!-- Global Fleet Progress -->
```
Juste après la balise fermante `</nav>`, ajouter:
```html
      <%= if length(@alerts) > 0 do %>
        <div class="fixed top-24 right-6 z-50 flex flex-col gap-2">
          <%= for alert <- @alerts do %>
            <div class="bg-red-500/20 border border-red-500 text-red-100 px-6 py-4 rounded-xl shadow-[0_0_20px_rgba(239,68,68,0.3)] backdrop-blur-md animate-pulse">
              <div class="flex items-center gap-3">
                <svg xmlns="http://www.w3.org/2000/svg" class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" /></svg>
                <span class="font-bold text-sm tracking-wide uppercase"><%= alert %></span>
              </div>
            </div>
          <% end %>
        </div>
      <% end %>
```

**Step 2: Commit**
```bash
git add src/dashboard/lib/axon_dashboard_web/live/status_live.ex
git commit -m "feat(dashboard): render proactive security alerts in real-time on UI"
```

---

### Task 3: Roadmap Update

**Files:**
- Modify: `ROADMAP.md`

**Step 1: Write minimal implementation**
Marquer la stratégie "Audit Proactif" comme terminée.

```markdown
- [x] **Audit Proactif :** Alerte automatique dès qu'un changement dégrade le score de sécurité.
```

**Step 2: Commit**
```bash
git add ROADMAP.md
git commit -m "docs: mark Proactive Audit phase as complete"
```