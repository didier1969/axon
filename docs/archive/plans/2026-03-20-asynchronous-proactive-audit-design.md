# Asynchronous Proactive Audit (Eventual Consistency Design)

## The Objective
To provide accurate, deeply analyzed Security (OWASP Taint Analysis) and Test Coverage scores for all projects in the Axon Dashboard without blocking the real-time file ingestion pipeline (Hot Path).

## The Approach (Option A - Eventual Consistency)
We will decouple the *ingestion* of structural data from the *analysis* of that data. The analysis (Audit) will be performed as a low-priority, background task when the system detects an "Idle" state (no active indexing).

### The Architecture: "The Idle Auditor"

1. **The Idle Detector (Elixir Control Plane):**
   - The `Axon.Watcher.Server` (Pod A) is the orchestrator. It knows when it is actively processing batches and when it has finished.
   - We will introduce an `idle_timer` in `Axon.Watcher.Server`.
   - Every time a file event is received or a batch is processed, the timer resets.
   - If the timer expires (e.g., 30 seconds of inactivity), the server transitions to an `:idle` state and fires a `TriggerAudit` event.

2. **The Audit Dispatcher (Elixir Background Job):**
   - When the `TriggerAudit` event fires, a background GenServer (or an Oban job) `Axon.Watcher.Auditor` picks it up.
   - It fetches the list of all indexed projects from the SQLite database.
   - For each project, it sends a JSON-RPC request (`axon_audit`) over the UNIX socket to the Rust Data Plane.

3. **The Graph Engine (Rust Data Plane):**
   - Rust (`axon-core`) receives the `axon_audit` request.
   - It executes the heavy KuzuDB Cypher queries for Taint Analysis and Coverage.
   - It returns the true, computed scores.

4. **State Reconciliation (The Dashboard):**
   - The Elixir `Auditor` receives the result.
   - It updates the SQLite database (`IndexedProject` table) with the true `security_score` and `coverage_score`.
   - It broadcasts a PubSub event (`{:audit_completed, project_name, security, coverage}`).
   - The `StatusLive` dashboard receives this event and updates its UI, flashing the new, accurate scores.

## Trade-offs
- **Pros:** Zero impact on file ingestion performance. CPU remains under the 70% threshold. Accurate metrics based on global graph topology.
- **Cons:** Scores on the dashboard are not "real-time". They reflect the state of the codebase ~30 seconds after the user stops typing.

## Implementation Steps
1. Create `Axon.Watcher.Auditor` (GenServer) to manage the background audit queue.
2. Add idle detection logic to `Axon.Watcher.Server`.
3. Ensure `BridgeClient` can handle outbound `axon_audit` calls and route responses back to the `Auditor`.
4. Update `StatusLive` to handle audit completion events.
