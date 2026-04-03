# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.CockpitLive do
  use Phoenix.LiveView

  alias Axon.Watcher.Progress
  alias Axon.Watcher.SqlGateway
  alias Axon.Watcher.Telemetry

  @tick_ms 750

  @impl true
  def mount(_params, _session, socket) do
    repo_slug = System.get_env("AXON_REPO_SLUG") || Path.expand(".") |> Path.basename()

    if connected?(socket) do
      :timer.send_interval(@tick_ms, self(), :tick)
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
    end

    socket =
      socket
      |> stream_configure(:projects, dom_id: &"project-#{slug_dom_id(&1.slug)}")
      |> stream_configure(:reasons, dom_id: &"reason-#{slug_dom_id(&1.reason)}")
      |> assign(
        repo_slug: repo_slug,
        monitoring_active: true,
        sql_source: SqlGateway.source_info(),
        workspace: default_workspace(),
        runtime: default_runtime(),
        recent_files: [],
        projects: [],
        reasons: [],
        readiness: default_readiness(),
        scan_complete: false,
        project_count: 0,
        reason_count: 0
      )
      |> assign_snapshot(build_snapshot(repo_slug))

    {:ok, socket}
  end

  @impl true
  def handle_info({:telemetry_event, _event, _measurements, _metadata}, socket) do
    {:noreply, socket}
  end

  @impl true
  def handle_info({:bridge_event, event}, socket) do
    {:noreply, apply_bridge_event(socket, event)}
  end

  @impl true
  def handle_info(:tick, socket) do
    {:noreply, refresh_snapshot(socket)}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="cockpit-shell">
      <header class="cockpit-header">
        <div>
          <p class="eyebrow">Workspace Control Plane</p>
          <h1>Axon Cockpit</h1>
          <p class="cockpit-subtitle">
            Read-only operational view over ingestion, backlog causality, project readiness and runtime health.
          </p>
        </div>

        <div class="header-signals">
          <.signal_chip label="Rust Runtime" value="Observed" tone={:ok} />
          <.signal_chip label="SQL Source" value={sql_source_value(@sql_source)} tone={:neutral} />
          <.signal_chip label="MCP" value={mcp_state(@workspace)} tone={mcp_tone(@workspace)} />
          <.signal_chip
            label="Truth"
            value={readiness_badge(@readiness.readiness_state)}
            tone={readiness_tone(@readiness.readiness_state)}
          />
        </div>
      </header>

      <section class="cockpit-band">
        <div class="band-title-row">
          <div>
            <p class="eyebrow">Workspace</p>
            <h2>Indexation globale</h2>
          </div>
          <div class="band-meta">
            <span>Repo slug: {@repo_slug}</span>
            <span>Completion: {@workspace["progress"]}%</span>
          </div>
        </div>

        <div class="hero-grid">
          <.metric_card label="Known Files" value={@workspace["known"]} tone={:neutral} />
          <.metric_card label="Completed" value={@workspace["completed"]} tone={:ok} />
          <.metric_card label="Graph Ready" value={@workspace["graph_ready"]} tone={:ok} />
          <.metric_card label="Vector Ready" value={@workspace["vector_ready"]} tone={:info} />
          <.metric_card label="Indexing" value={@workspace["indexing"]} tone={:info} />
          <.metric_card label="Pending" value={@workspace["pending"]} tone={:warn} />
          <.metric_card label="Degraded" value={@workspace["indexed_degraded"]} tone={:warn} />
          <.metric_card label="Oversized" value={@workspace["oversized"]} tone={:danger} />
          <.metric_card label="Skipped" value={@workspace["skipped"]} tone={:neutral} />
          <.metric_card label="Deleted" value={@workspace["deleted"]} tone={:neutral} />
        </div>

        <div class="progress-rail">
          <div class="progress-rail-fill" style={"width: #{@workspace["progress"]}%"}></div>
        </div>
      </section>

      <div class="cockpit-columns cockpit-columns-workbench">
        <div class="cockpit-column-stack">
          <section class="cockpit-band">
            <div class="band-title-row">
              <div>
                <p class="eyebrow">Backlog</p>
                <h2>Causes dominantes</h2>
              </div>
              <span class="band-kicker">{backlog_summary(@workspace)}</span>
            </div>

            <div :if={@reason_count == 0} class="empty-state">
              No dominant backlog cause is visible yet.
            </div>

            <div :if={@reason_count > 0} id="backlog-reasons" phx-update="stream" class="stack-list">
              <div :for={{dom_id, reason} <- @streams.reasons} id={dom_id} class="stack-row">
                <div>
                  <p class="stack-title">{reason.label}</p>
                  <p class="stack-caption">{reason.reason}</p>
                </div>
                <span class="stack-value">{reason.count}</span>
              </div>
            </div>

            <div class="mini-grid">
              <.signal_stat label="Ready Truth" value={readiness_badge(@readiness.readiness_state)} />
              <.signal_stat label="Coverage" value={"#{@workspace["progress"]}%"} />
              <.signal_stat label="Visible Scope" value={Integer.to_string(@workspace["known"])} />
              <.signal_stat label="Hot Backlog" value={Integer.to_string(@workspace["indexing"])} />
              <.signal_stat label="Promoted" value={Integer.to_string(@workspace["stage_promoted"])} />
              <.signal_stat label="Claimed" value={Integer.to_string(@workspace["stage_claimed"])} />
              <.signal_stat
                label="Writer Pending"
                value={Integer.to_string(@workspace["stage_writer_pending_commit"])}
              />
              <.signal_stat
                label="Graph Indexed"
                value={Integer.to_string(@workspace["stage_graph_indexed"])}
              />
            </div>
          </section>

          <section class="cockpit-band">
            <div class="band-title-row">
              <div>
                <p class="eyebrow">Runtime</p>
                <h2>Service & scheduling</h2>
              </div>
              <span class="band-kicker">{String.upcase(@runtime.claim_mode)}</span>
            </div>

            <div class="detail-grid">
              <.signal_stat label="Queue Depth" value={Integer.to_string(@runtime.queue_depth)} />
              <.signal_stat
                label="Graph Projection Queued"
                value={Integer.to_string(@runtime.graph_projection_queue_queued)}
              />
              <.signal_stat
                label="Graph Projection In-Flight"
                value={Integer.to_string(@runtime.graph_projection_queue_inflight)}
              />
            <.signal_stat
                label="Graph Projection Pending"
                value={Integer.to_string(@runtime.graph_projection_queue_depth)}
              />
              <.signal_stat
                label="File Vector Queued"
                value={Integer.to_string(@runtime.file_vectorization_queue_queued)}
              />
              <.signal_stat
                label="File Vector In-Flight"
                value={Integer.to_string(@runtime.file_vectorization_queue_inflight)}
              />
              <.signal_stat
                label="File Vector Pending"
                value={Integer.to_string(@runtime.file_vectorization_queue_depth)}
              />
              <.signal_stat label="Claim Mode" value={String.upcase(@runtime.claim_mode)} />
              <.signal_stat
                label="Service Pressure"
                value={String.upcase(@runtime.service_pressure)}
              />
              <.signal_stat label="Bridge" value={bridge_status_label(@runtime.bridge_status)} />
              <.signal_stat
                label="SQL Snapshot"
                value={sql_status_label(@runtime.sql_snapshot_status, @runtime.sql_snapshot_last_duration_ms)}
              />
              <.signal_stat
                label="Budget Reserved"
                value={"#{format_mib(@runtime.reserved_bytes)} MB / #{format_mib(@runtime.budget_bytes)} MB"}
              />
              <.signal_stat
                label="Budget Exhaustion"
                value={"#{Float.round(@runtime.exhaustion_ratio * 100, 1)}%"}
              />
              <.signal_stat
                label="Reserved Tasks"
                value={Integer.to_string(@runtime.reserved_task_count)}
              />
              <.signal_stat
                label="Anon Trace Reserved"
                value={Integer.to_string(@runtime.anonymous_trace_reserved_tasks)}
              />
              <.signal_stat
                label="Anon Trace Total"
                value={Integer.to_string(@runtime.anonymous_trace_admissions_total)}
              />
              <.signal_stat
                label="Release Misses"
                value={Integer.to_string(@runtime.reservation_release_misses_total)}
              />
              <.signal_stat
                label="Oversized"
                value={Integer.to_string(@runtime.oversized_refusals_total)}
              />
              <.signal_stat
                label="Degraded"
                value={Integer.to_string(@runtime.degraded_mode_entries_total)}
              />
              <.signal_stat
                label="Guidance Slots"
                value={"#{@runtime.host_guidance_slots} slots"}
              />
              <.signal_stat label="Host CPU" value={"#{format_float(@runtime.cpu_load)}%"} />
              <.signal_stat label="Host RAM" value={"#{format_float(@runtime.ram_load)}%"} />
              <.signal_stat label="Host IO Wait" value={"#{format_float(@runtime.io_wait)}%"} />
              <.signal_stat label="Host State" value={String.upcase(@runtime.host_state)} />
            </div>
          </section>

          <section class="cockpit-band">
            <div class="band-title-row">
              <div>
                <p class="eyebrow">Ingress</p>
                <h2>Buffer & promotion</h2>
              </div>
              <span class="band-kicker">
                {if @runtime.ingress_enabled, do: "enabled", else: "disabled"}
              </span>
            </div>

            <div class="detail-grid">
              <.signal_stat
                label="Buffered Entries"
                value={Integer.to_string(@runtime.ingress_buffered_entries)}
              />
              <.signal_stat
                label="Subtree Hints"
                value={Integer.to_string(@runtime.ingress_subtree_hints)}
              />
              <.signal_stat
                label="Hint In Flight"
                value={Integer.to_string(@runtime.ingress_subtree_hint_in_flight)}
              />
              <.signal_stat
                label="Hint Accepted"
                value={Integer.to_string(@runtime.ingress_subtree_hint_accepted_total)}
              />
              <.signal_stat
                label="Hint Blocked"
                value={Integer.to_string(@runtime.ingress_subtree_hint_blocked_total)}
              />
              <.signal_stat
                label="Hint Suppressed"
                value={Integer.to_string(@runtime.ingress_subtree_hint_suppressed_total)}
              />
              <.signal_stat
                label="Collapsed Total"
                value={Integer.to_string(@runtime.ingress_collapsed_total)}
              />
              <.signal_stat
                label="Flush Count"
                value={Integer.to_string(@runtime.ingress_flush_count)}
              />
              <.signal_stat
                label="Last Flush"
                value={"#{@runtime.ingress_last_flush_duration_ms} ms"}
              />
              <.signal_stat
                label="Last Promoted"
                value={Integer.to_string(@runtime.ingress_last_promoted_count)}
              />
            </div>
          </section>

          <section class="cockpit-band">
            <div class="band-title-row">
              <div>
                <p class="eyebrow">Memory</p>
                <h2>Heap, file pages & DuckDB</h2>
              </div>
              <span class="band-kicker">{memory_dominant(@runtime)}</span>
            </div>

            <div class="detail-grid">
              <.signal_stat label="RSS" value={"#{format_mib(@runtime.rss_bytes)} MB"} />
              <.signal_stat label="RssAnon" value={"#{format_mib(@runtime.rss_anon_bytes)} MB"} />
              <.signal_stat label="RssFile" value={"#{format_mib(@runtime.rss_file_bytes)} MB"} />
              <.signal_stat label="RssShmem" value={"#{format_mib(@runtime.rss_shmem_bytes)} MB"} />
              <.signal_stat label="DuckDB DB" value={"#{format_mib(@runtime.db_file_bytes)} MB"} />
              <.signal_stat label="DuckDB WAL" value={"#{format_mib(@runtime.db_wal_bytes)} MB"} />
              <.signal_stat label="DuckDB Total" value={"#{format_mib(@runtime.db_total_bytes)} MB"} />
              <.signal_stat
                label="DuckDB Memory"
                value={"#{format_mib(@runtime.duckdb_memory_bytes)} MB"}
              />
              <.signal_stat
                label="DuckDB Temp"
                value={"#{format_mib(@runtime.duckdb_temporary_bytes)} MB"}
              />
            </div>
          </section>
        </div>

        <section class="cockpit-band project-band">
          <div class="band-title-row">
            <div>
              <p class="eyebrow">Projects</p>
              <h2>Readiness par projet</h2>
            </div>
            <span class="band-kicker">{@readiness.project_summary}</span>
          </div>

          <div :if={@project_count == 0} class="empty-state">
            No project snapshot is available yet.
          </div>

          <div
            :if={@project_count > 0}
            id="project-readiness"
            phx-update="stream"
            class="stack-list"
          >
            <div :for={{dom_id, project} <- @streams.projects} id={dom_id} class="project-card">
              <div class="project-head">
                <div>
                  <p class="stack-title">{project.slug}</p>
                  <p class="stack-caption">{project.readiness |> String.upcase()} readiness</p>
                </div>
                <span class={["readiness-pill", readiness_class(project.readiness)]}>
                  {project.progress}%
                </span>
              </div>

              <div class="project-grid">
                <.signal_stat label="Known Files" value={Integer.to_string(project.known)} />
                <.signal_stat label="Completed" value={Integer.to_string(project.completed)} />
                <.signal_stat label="Pending" value={Integer.to_string(project.pending)} />
                <.signal_stat label="Indexing" value={Integer.to_string(project.indexing)} />
                <.signal_stat label="Degraded" value={Integer.to_string(project.degraded)} />
              </div>

              <div class="progress-rail compact">
                <div class="progress-rail-fill" style={"width: #{project.progress}%"}></div>
              </div>
            </div>
          </div>
        </section>
      </div>

      <section class="cockpit-band full-span">
        <div class="band-title-row">
          <div>
            <p class="eyebrow">Recent Activity</p>
            <h2>Latest confirmed files</h2>
          </div>
          <span :if={@scan_complete} class="band-kicker success">
            Runtime reported scan completion
          </span>
        </div>

        <div class="activity-list">
          <div :if={Enum.empty?(@recent_files)} class="empty-state">
            Waiting for recent bridge events.
          </div>

          <div :if={not Enum.empty?(@recent_files)} class="activity-table">
            <div class="activity-header">
              <span>Status</span>
              <span>Size</span>
              <span>Extension</span>
              <span>Full Path</span>
              <span>Time</span>
            </div>

            <div :for={file <- @recent_files} class="activity-row">
              <span class={["activity-badge", activity_class(file.status)]}>
                {activity_label(file.status)}
              </span>
              <span class="activity-size">{activity_size_label(file.size_bytes)}</span>
              <span class="activity-extension">{activity_extension_label(file.extension)}</span>
              <span class="activity-path">{file.path}</span>
              <span class="activity-time">{activity_time_label(file.time)}</span>
            </div>
          </div>
        </div>
      </section>
    </div>
    """
  end

  attr :label, :string, required: true
  attr :value, :string, required: true
  attr :tone, :atom, default: :neutral

  defp signal_chip(assigns) do
    ~H"""
    <div class={["signal-chip", tone_class(@tone)]}>
      <span class="signal-chip-label">{@label}</span>
      <strong>{@value}</strong>
    </div>
    """
  end

  attr :label, :string, required: true
  attr :value, :any, required: true
  attr :tone, :atom, default: :neutral

  defp metric_card(assigns) do
    ~H"""
    <article class={["metric-card", tone_class(@tone)]}>
      <p class="metric-label">{@label}</p>
      <p class="metric-value">{@value}</p>
    </article>
    """
  end

  attr :label, :string, required: true
  attr :value, :string, required: true

  defp signal_stat(assigns) do
    ~H"""
    <div class="signal-stat">
      <span class="signal-stat-label">{@label}</span>
      <strong class="signal-stat-value">{@value}</strong>
    </div>
    """
  end

  defp assign_snapshot(socket, snapshot) do
    socket
      |> assign(
        sql_source: snapshot.sql_source,
        workspace: snapshot.workspace,
        runtime: snapshot.runtime,
        readiness: snapshot.readiness,
        recent_files: snapshot.recent_files,
        projects: snapshot.projects,
        reasons: snapshot.reasons,
        project_count: length(snapshot.projects),
        reason_count: length(snapshot.reasons)
      )
    |> stream(:projects, snapshot.projects, reset: true)
    |> stream(:reasons, snapshot.reasons, reset: true)
  end

  defp refresh_snapshot(socket) do
    snapshot = build_snapshot(socket.assigns.repo_slug)

    snapshot =
      snapshot
      |> preserve_workspace(socket.assigns.workspace)
      |> preserve_projects(socket.assigns.project_count, socket.assigns.projects)
      |> preserve_reasons(socket.assigns.reason_count, socket.assigns.reasons)

    assign_snapshot(socket, snapshot)
  end

  defp build_snapshot(repo_slug) do
    progress_snapshot = Progress.get_snapshot(repo_slug)
    workspace = progress_snapshot.workspace
    projects = progress_snapshot.projects
    reasons = progress_snapshot.reasons
    runtime = structify_runtime(Telemetry.get_stats())

    %{
      sql_source: SqlGateway.source_info(),
      workspace: workspace,
      projects: projects,
      reasons: reasons,
      runtime: runtime,
      readiness: derive_readiness(workspace, projects),
      recent_files: Map.get(runtime, :last_files, [])
    }
  end

  defp preserve_workspace(%{workspace: workspace} = snapshot, previous_workspace) do
    if zero_workspace?(workspace) and not zero_workspace?(previous_workspace) do
      %{snapshot | workspace: previous_workspace}
    else
      snapshot
    end
  end

  defp preserve_projects(%{projects: []} = snapshot, previous_count, previous_projects)
       when previous_count > 0 do
    %{snapshot | projects: previous_projects}
  end

  defp preserve_projects(snapshot, _previous_count, _previous_projects), do: snapshot

  defp preserve_reasons(%{reasons: []} = snapshot, previous_count, previous_reasons)
       when previous_count > 0 do
    %{snapshot | reasons: previous_reasons}
  end

  defp preserve_reasons(snapshot, _previous_count, _previous_reasons), do: snapshot

  defp zero_workspace?(workspace) when is_map(workspace) do
    Enum.all?(
      ["known", "completed", "pending", "indexing", "indexed", "indexed_degraded", "skipped"],
      fn key -> Map.get(workspace, key, 0) == 0 end
    )
  end

  defp apply_bridge_event(socket, %{"FileIndexed" => payload}) do
    path = Map.get(payload, "path", "unknown")
    status = file_status(Map.get(payload, "status", "ok"))
    file = recent_activity_entry(path, status, DateTime.utc_now())

    assign(socket,
      recent_files: [file | Enum.take(socket.assigns.recent_files, 11)],
      scan_complete: false
    )
  end

  defp apply_bridge_event(socket, %{"ScanComplete" => _payload}) do
    assign(socket, scan_complete: true)
  end

  defp apply_bridge_event(socket, %{"RuntimeTelemetry" => payload}) do
    Telemetry.update_runtime_telemetry(payload)

    runtime =
      Telemetry.get_stats()
      |> structify_runtime()

    assign(socket, runtime: runtime)
  end

  defp apply_bridge_event(socket, _event), do: socket

  defp derive_readiness(workspace, projects) do
    ready =
      Enum.count(projects, fn project ->
        project.readiness == "ready"
      end)

    partial =
      Enum.count(projects, fn project ->
        project.readiness in ["partial", "warming"]
      end)

    queued =
      Enum.count(projects, fn project ->
        project.readiness == "queued"
      end)

    state =
      cond do
        workspace["completed"] == 0 -> "cold"
        workspace["progress"] >= 90 and workspace["indexed_degraded"] == 0 -> "ready"
        workspace["completed"] > 0 -> "partial"
        true -> "cold"
      end

    %{
      readiness_state: state,
      project_summary: "#{ready} ready, #{partial} partial, #{queued} queued"
    }
  end

  defp default_workspace do
    %{
      "status" => "connecting",
      "progress" => 0,
      "synced" => 0,
      "total" => 0,
      "indexed" => 0,
      "indexed_degraded" => 0,
      "pending" => 0,
      "indexing" => 0,
      "oversized" => 0,
      "skipped" => 0,
      "deleted" => 0,
      "graph_ready" => 0,
      "vector_ready" => 0,
      "stage_promoted" => 0,
      "stage_claimed" => 0,
      "stage_writer_pending_commit" => 0,
      "stage_graph_indexed" => 0,
      "known" => 0,
      "completed" => 0
    }
  end

  defp default_runtime do
    Telemetry.get_stats()
    |> structify_runtime()
  end

  defp default_readiness do
    %{readiness_state: "cold", project_summary: "0 ready, 0 partial, 0 queued"}
  end

  defp structify_runtime(stats) do
    %{
      budget_bytes: Map.get(stats, :budget_bytes, 0) || 0,
      reserved_bytes: Map.get(stats, :reserved_bytes, 0) || 0,
      exhaustion_ratio: Map.get(stats, :exhaustion_ratio, 0.0) || 0.0,
      reserved_task_count: Map.get(stats, :reserved_task_count, 0) || 0,
      anonymous_trace_reserved_tasks: Map.get(stats, :anonymous_trace_reserved_tasks, 0) || 0,
      anonymous_trace_admissions_total:
        Map.get(stats, :anonymous_trace_admissions_total, 0) || 0,
      reservation_release_misses_total:
        Map.get(stats, :reservation_release_misses_total, 0) || 0,
      queue_depth: Map.get(stats, :queue_depth, 0) || 0,
      claim_mode: Map.get(stats, :claim_mode, "unknown") || "unknown",
      service_pressure: Map.get(stats, :service_pressure, "healthy") || "healthy",
      oversized_refusals_total: Map.get(stats, :oversized_refusals_total, 0) || 0,
      degraded_mode_entries_total: Map.get(stats, :degraded_mode_entries_total, 0) || 0,
      cpu_load: Map.get(stats, :cpu_load, 0.0) || 0.0,
      ram_load: Map.get(stats, :ram_load, 0.0) || 0.0,
      io_wait: Map.get(stats, :io_wait, 0.0) || 0.0,
      host_state: Map.get(stats, :host_state, "healthy") || "healthy",
      host_guidance_slots: Map.get(stats, :host_guidance_slots, 0) || 0,
      rss_bytes: Map.get(stats, :rss_bytes, 0) || 0,
      rss_anon_bytes: Map.get(stats, :rss_anon_bytes, 0) || 0,
      rss_file_bytes: Map.get(stats, :rss_file_bytes, 0) || 0,
      rss_shmem_bytes: Map.get(stats, :rss_shmem_bytes, 0) || 0,
      db_file_bytes: Map.get(stats, :db_file_bytes, 0) || 0,
      db_wal_bytes: Map.get(stats, :db_wal_bytes, 0) || 0,
      db_total_bytes: Map.get(stats, :db_total_bytes, 0) || 0,
      duckdb_memory_bytes: Map.get(stats, :duckdb_memory_bytes, 0) || 0,
      duckdb_temporary_bytes: Map.get(stats, :duckdb_temporary_bytes, 0) || 0,
      graph_projection_queue_queued:
        Map.get(stats, :graph_projection_queue_queued, 0) || 0,
      graph_projection_queue_inflight:
        Map.get(stats, :graph_projection_queue_inflight, 0) || 0,
      graph_projection_queue_depth:
        Map.get(stats, :graph_projection_queue_depth, 0) || 0,
      file_vectorization_queue_queued:
        Map.get(stats, :file_vectorization_queue_queued, 0) || 0,
      file_vectorization_queue_inflight:
        Map.get(stats, :file_vectorization_queue_inflight, 0) || 0,
      file_vectorization_queue_depth:
        Map.get(stats, :file_vectorization_queue_depth, 0) || 0,
      ingress_enabled: Map.get(stats, :ingress_enabled, false) || false,
      ingress_buffered_entries: Map.get(stats, :ingress_buffered_entries, 0) || 0,
      ingress_subtree_hints: Map.get(stats, :ingress_subtree_hints, 0) || 0,
      ingress_subtree_hint_in_flight:
        Map.get(stats, :ingress_subtree_hint_in_flight, 0) || 0,
      ingress_subtree_hint_accepted_total:
        Map.get(stats, :ingress_subtree_hint_accepted_total, 0) || 0,
      ingress_subtree_hint_blocked_total:
        Map.get(stats, :ingress_subtree_hint_blocked_total, 0) || 0,
      ingress_subtree_hint_suppressed_total:
        Map.get(stats, :ingress_subtree_hint_suppressed_total, 0) || 0,
      ingress_collapsed_total: Map.get(stats, :ingress_collapsed_total, 0) || 0,
      ingress_flush_count: Map.get(stats, :ingress_flush_count, 0) || 0,
      ingress_last_flush_duration_ms: Map.get(stats, :ingress_last_flush_duration_ms, 0) || 0,
      ingress_last_promoted_count: Map.get(stats, :ingress_last_promoted_count, 0) || 0,
      last_files: Map.get(stats, :last_files, []) || [],
      bridge_status: Map.get(stats, :bridge_status, :connecting) || :connecting,
      bridge_last_connected_at: Map.get(stats, :bridge_last_connected_at, nil),
      bridge_last_disconnected_at: Map.get(stats, :bridge_last_disconnected_at, nil),
      sql_snapshot_status: Map.get(stats, :sql_snapshot_status, :unknown) || :unknown,
      sql_snapshot_last_success_at: Map.get(stats, :sql_snapshot_last_success_at, nil),
      sql_snapshot_last_error_at: Map.get(stats, :sql_snapshot_last_error_at, nil),
      sql_snapshot_last_error_reason: Map.get(stats, :sql_snapshot_last_error_reason, nil),
      sql_snapshot_last_duration_ms: Map.get(stats, :sql_snapshot_last_duration_ms, 0) || 0
    }
  end

  defp backlog_summary(workspace) do
    "#{workspace["pending"]} pending, #{workspace["indexing"]} indexing"
  end

  defp mcp_state(workspace) do
    cond do
      workspace["completed"] == 0 -> "Cold"
      workspace["progress"] >= 90 -> "Ready"
      true -> "Partial"
    end
  end

  defp mcp_tone(workspace) do
    cond do
      workspace["completed"] == 0 -> :warn
      workspace["progress"] >= 90 -> :ok
      true -> :info
    end
  end

  defp readiness_badge("ready"), do: "Ready"
  defp readiness_badge("partial"), do: "Partial"
  defp readiness_badge("cold"), do: "Cold"
  defp readiness_badge(_), do: "Warming"

  defp bridge_status_label(:connected), do: "CONNECTED"
  defp bridge_status_label(:disconnected), do: "DISCONNECTED"
  defp bridge_status_label(:connecting), do: "CONNECTING"
  defp bridge_status_label(other), do: other |> to_string() |> String.upcase()

  defp sql_status_label(:ok, duration_ms), do: "OK (#{duration_ms} ms)"
  defp sql_status_label(:error, duration_ms), do: "ERROR (#{duration_ms} ms)"
  defp sql_status_label(:unknown, duration_ms), do: "UNKNOWN (#{duration_ms} ms)"
  defp sql_status_label(other, duration_ms),
    do: "#{other |> to_string() |> String.upcase()} (#{duration_ms} ms)"

  defp sql_source_value(%{endpoint: endpoint}) when is_binary(endpoint), do: endpoint
  defp sql_source_value(_), do: "unknown"

  defp readiness_tone("ready"), do: :ok
  defp readiness_tone("partial"), do: :info
  defp readiness_tone("cold"), do: :warn
  defp readiness_tone(_), do: :neutral

  defp readiness_class("ready"), do: "ready"
  defp readiness_class("partial"), do: "partial"
  defp readiness_class("warming"), do: "warming"
  defp readiness_class(_), do: "queued"

  defp tone_class(:ok), do: "tone-ok"
  defp tone_class(:info), do: "tone-info"
  defp tone_class(:warn), do: "tone-warn"
  defp tone_class(:danger), do: "tone-danger"
  defp tone_class(_), do: "tone-neutral"

  defp format_mib(bytes) when is_integer(bytes), do: div(bytes, 1_048_576)
  defp format_mib(bytes) when is_float(bytes), do: round(bytes / 1_048_576)
  defp format_mib(_bytes), do: 0

  defp format_float(value) when is_float(value), do: Float.round(value, 1)
  defp format_float(value) when is_integer(value), do: (value * 1.0) |> Float.round(1)
  defp format_float(_value), do: 0.0

  defp memory_dominant(runtime) do
    cond do
      runtime.rss_anon_bytes >= runtime.rss_file_bytes -> "anonymous memory dominates"
      true -> "file-backed memory dominates"
    end
  end

  defp file_status("ok"), do: :ok
  defp file_status("indexed_degraded"), do: :degraded
  defp file_status(_), do: :error

  defp activity_label(:ok), do: "SUCCESS"
  defp activity_label(:degraded), do: "DEGRADED"
  defp activity_label(_), do: "ERROR"

  defp activity_class(:ok), do: "ok"
  defp activity_class(:degraded), do: "degraded"
  defp activity_class(_), do: "error"

  defp activity_size_label(size_bytes) when is_integer(size_bytes) and size_bytes >= 1_048_576,
    do: "#{Float.round(size_bytes / 1_048_576, 1)} MB"

  defp activity_size_label(size_bytes) when is_integer(size_bytes) and size_bytes >= 1024,
    do: "#{Float.round(size_bytes / 1024, 1)} KB"

  defp activity_size_label(size_bytes) when is_integer(size_bytes) and size_bytes >= 0,
    do: "#{size_bytes} B"

  defp activity_size_label(_), do: "n/a"

  defp activity_extension_label(extension) when extension in [nil, "", "."], do: "(none)"
  defp activity_extension_label(extension), do: extension

  defp activity_time_label(%DateTime{} = time), do: String.slice(DateTime.to_iso8601(time), 11, 8)
  defp activity_time_label(_), do: "--:--:--"

  defp recent_activity_entry(path, status, time) do
    %{
      path: path,
      status: status,
      time: time,
      extension: path |> Path.extname() |> String.trim_leading(".") |> blank_to_none(),
      size_bytes: recent_activity_file_size(path)
    }
  end

  defp recent_activity_file_size(path) do
    case File.stat(path) do
      {:ok, stat} -> stat.size
      {:error, _reason} -> nil
    end
  end

  defp blank_to_none(""), do: "(none)"
  defp blank_to_none(value), do: value

  defp slug_dom_id(value) do
    value
    |> to_string()
    |> String.downcase()
    |> String.replace(~r/[^a-z0-9]+/u, "-")
    |> String.trim("-")
    |> case do
      "" -> "unknown"
      dom_id -> dom_id
    end
  end
end
