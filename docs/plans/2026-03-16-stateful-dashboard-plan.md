# Implementation Plan: Stateful Dashboard using Ecto (indexed_projects / indexed_files)

## Goal
Make the Axon dashboard fully stateful and persistent. Instead of relying on ephemeral PubSub events and volatile ETS tables, the system will use the newly created SQLite tables (`indexed_projects` and `indexed_files`) as the single source of truth. The dashboard will query these tables on mount and update them dynamically via PubSub events.

## Tasks

### Task 1: Create Ecto Schemas for the New Tables
- Create `src/dashboard/lib/axon_nexus/axon/watcher/indexed_project.ex` mapped to the `indexed_projects` table.
- Create `src/dashboard/lib/axon_nexus/axon/watcher/indexed_file.ex` mapped to the `indexed_files` table.
- Ensure proper relationships (`has_many :files`, `belongs_to :project`) and changesets are defined.

### Task 2: Implement Database Context (The API)
- Create `src/dashboard/lib/axon_nexus/axon/watcher/tracking.ex` (or update an existing context) to expose functions for upserting projects and files.
- Functions needed: 
  - `upsert_project!(name, path, status \\ "active")`
  - `upsert_file!(project_id, path, file_hash)`
  - `mark_file_status!(path, status, params \\ %{})` (e.g., status: "indexed", "failed", "ignored_by_rule", params: %{symbols_count: 10})
  - `get_dashboard_stats()`: Returns an aggregated map of projects, their file counts (total, indexed, failed), and global totals to replace the old ETS query.

### Task 3: Hook Server into Database (Discovery Phase)
- Update `src/dashboard/lib/axon_nexus/axon/watcher/server.ex`.
- When scanning the directory in `handle_info({:ok, path}, state)` or `should_process?`:
  - Determine the project name/path (e.g., top-level directory in `~/projects/`).
  - Upsert the project into `indexed_projects`.
  - Upsert the file into `indexed_files` with `status: "pending"`.
  - If a file is skipped due to `.axonignore` or hash match, mark it appropriately in the DB (or leave it as is if hash matches).

### Task 4: Hook Workers into Database (Processing Phase)
- Update `src/dashboard/lib/axon_nexus/axon/watcher/indexing_worker.ex` (or `PoolFacade`/`BridgeClient` depending on where the result is caught). 
- When `PoolFacade.parse` returns `:ok`, call `Tracking.mark_file_status!(file_path, "indexed", %{symbols_count: count})` (if count is available, else just "indexed").
- If it returns `{:error, reason}`, call `Tracking.mark_file_status!(file_path, "failed", %{error_reason: inspect(reason)})`.
- *Note: You may need to adapt `Axon.Watcher.Telemetry` to rely on the DB, or phase it out in favor of `Tracking`.*

### Task 5: Refactor StatusLive Dashboard to Use Ecto
- Update `src/dashboard/lib/axon_dashboard_web/live/status_live.ex`.
- In `mount/3`, call `Tracking.get_dashboard_stats()` instead of `Telemetry.get_stats()` to hydrate the initial state perfectly.
- Update the `handle_info` callbacks to trigger a re-fetch of `Tracking.get_dashboard_stats()` (or selectively update state) so the UI stays in sync with the DB.
- Ensure the UI renders the new metrics (`failed_files`, `ignored_files`, etc.).