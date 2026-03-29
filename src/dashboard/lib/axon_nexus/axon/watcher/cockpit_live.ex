defmodule Axon.Watcher.CockpitLive do
  use Phoenix.LiveView, layout: {Axon.Watcher.Layouts, :root}
  alias Axon.Watcher.Progress

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      :timer.send_interval(500, self(), :tick)
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "telemetry_events")
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
    end

    repo_slug = System.get_env("AXON_REPO_SLUG") || Path.expand(".") |> Path.basename()
    monitoring_active = Axon.Watcher.Server.get_monitoring_status()

    {:ok,
     assign(socket,
       repo_slug: repo_slug,
       stats: %{},
       dir_stats: %{},
       monitoring_active: monitoring_active,
       live: %{active_workers: %{}, last_files: [], total_ingested: 0, directories: %{}, target_pressure: 100, t4_ema: 0.0, flux_reel: 0.0}
     )}
  end

  @impl true
  def handle_info({:backpressure_update, data}, socket) do
    live = Map.merge(socket.assigns.live, %{target_pressure: data.pressure, t4_ema: data.t4_ema})
    {:noreply, assign(socket, live: live)}
  end

  # NEXUS V5.6: We no longer handle individual telemetry events in LiveView
  # to prevent rendering saturation during high-speed ingestion (> 100 f/s).
  # The 500ms :tick is enough to keep the UI fresh without killing the BEAM scheduler.
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
    stats = Progress.get_status(socket.assigns.repo_slug)
    dir_stats = Progress.get_directory_stats(socket.assigns.repo_slug)
    monitoring_active = Axon.Watcher.Server.get_monitoring_status()

    live =
      Axon.Watcher.Telemetry.get_stats()
      |> Map.merge(%{
        total_files: stats["total"] || 0,
        total_ingested: stats["synced"] || 0,
        indexing_progress: stats["progress"] || 0,
        directories: dir_stats
      })

    {:noreply,
     assign(socket,
       stats: stats,
       dir_stats: dir_stats,
       monitoring_active: monitoring_active,
       live: live
     )}
  end

  @impl true
  def handle_event("start_scan", _params, socket) do
    require Logger
    Logger.info("[Cockpit] User triggered START_SCAN")
    Axon.Watcher.Server.trigger_scan()
    {:noreply, put_flash(socket, :info, "Full scan triggered!")}
  end

  @impl true
  def handle_event("toggle_monitoring", _params, socket) do
    require Logger

    if socket.assigns.monitoring_active do
      Logger.info("[Cockpit] User triggered PAUSE_MONITORING")
      Axon.Watcher.Server.pause_monitoring()
    else
      Logger.info("[Cockpit] User triggered RESUME_MONITORING")
      Axon.Watcher.Server.resume_monitoring()
    end

    {:noreply, socket}
  end

  @impl true
  def handle_event("purge_data", _params, socket) do
    require Logger
    Logger.info("[Cockpit] User triggered PURGE_DATA")
    Axon.Watcher.Server.purge_data()
    {:noreply, put_flash(socket, :error, "Knowledge base purged!")}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="header">
      <div class="logo">
        AXON <span style="font-weight: 400; color: var(--text-dim);">SYSTEMS</span>
        <div style="font-size: 0.75rem; color: var(--text-dim); margin-top: 4px;">
          Multi-Project Control Plane
        </div>
      </div>
      <div style="display:flex; gap: 12px; align-items: center;">
        <div class={"status-badge #{if @monitoring_active, do: "status-live", else: "status-error"}"}>
          {if @monitoring_active, do: "● MONITORING ACTIVE", else: "○ MONITORING PAUSED"}
        </div>
        <div class="pulse"></div>
      </div>
    </div>

    <div class="grid">
      <!-- Unit 01: Core Watcher -->
      <div class="card">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24">
            <path
              fill="currentColor"
              d="M12,9A3,3 0 0,0 9,12A3,3 0 0,0 12,15A3,3 0 0,0 15,12A3,3 0 0,0 12,9M12,17A5,5 0 0,1 7,12A5,5 0 0,1 12,7A5,5 0 0,1 17,12A5,5 0 0,1 12,17M12,4.5C7,4.5 2.73,7.61 1,12C2.73,16.39 7,19.5 12,19.5C17,19.5 21.27,16.39 23,12C21.27,7.61 17,4.5 12,4.5Z"
            />
          </svg>
          UNIT 01: CORE WATCHER
        </div>
        <div class="stat"><label>REPOSITORY</label> <span>{@repo_slug}</span></div>
        <div class="stat">
          <label>STATUS</label>
          <span style="color: var(--neon-green);">{String.upcase(@stats["status"] || "live")}</span>
        </div>
        <div class="stat">
          <label>TOTAL_INGESTED</label>
          <span style="color: var(--neon-blue);">{@live.total_ingested}</span>
        </div>

        <div class="progress-bar">
          <div class="progress-fill" style={"width: #{@stats["progress"] || 0}%"}></div>
        </div>
        <div style="display:flex; justify-content: space-between; margin-top: 10px; font-size: 0.7rem; font-weight: 700;">
          <span style="color: var(--text-dim);">PIPELINE_LOAD</span>
          <span style="color: var(--neon-blue);">{@stats["progress"] || 0}%</span>
        </div>
      </div>
      
    <!-- Unit 02: Parser Matrix (POD B) -->
      <div class="card">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24">
            <path
              fill="currentColor"
              d="M21,16.5C21,16.88 20.79,17.21 20.47,17.38L12.57,21.82C12.41,21.94 12.21,22 12,22C11.79,22 11.59,21.94 11.43,21.82L3.53,17.38C3.21,17.21 3,16.88 3,16.5V7.5C3,7.12 3.21,6.79 3.53,6.62L11.43,2.18C11.59,2.06 11.79,2 12,2C12.21,2 12.41,2.06 12.57,2.18L20.47,6.62C20.79,6.79 21,7.12 21,7.5V16.5Z"
            />
          </svg>
          UNIT 02: PARSER MATRIX (POD B)
        </div>
        <div style="display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px;">
          <%= for i <- 1..8 do %>
            <% worker = @live.active_workers["oban:#{i}"] %>
            <div style={"height: 40px; border: 1px solid #{if worker, do: "var(--neon-green)", else: "var(--border)"}; background: #{if worker, do: "rgba(0,255,65,0.05)", else: "transparent"}; display: flex; align-items: center; justify-content: center; position: relative;"}>
              <div :if={worker} class="pulse" style="position: absolute; top: 4px; right: 4px;"></div>
              <span style="font-size: 0.6rem; color: var(--text-dim);">W{i}</span>
            </div>
          <% end %>
        </div>
        <div style="margin-top: 15px;">
          <%= if map_size(@live.active_workers) > 0 do %>
            <div style="font-size: 0.7rem; color: var(--neon-green); font-family: monospace; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
              >> PARSING: {(Map.values(@live.active_workers) |> List.first()).file}
            </div>
          <% else %>
            <div style="font-size: 0.7rem; color: var(--text-dim); font-family: monospace;">
              IDLE_WAITING_FOR_TASKS
            </div>
          <% end %>
        </div>
      </div>
      
      <!-- Unit 03: Operational Override -->
      <div class="card" style="border-color: var(--neon-blue);">
        <div class="card-title" style="color: var(--neon-blue);">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24">
            <path
              fill="currentColor"
              d="M12,15.5A2.5,2.5 0 0,1 14.5,18A2.5,2.5 0 0,1 12,20.5A2.5,2.5 0 0,1 9.5,18A2.5,2.5 0 0,1 12,15.5M12,2A3,3 0 0,1 15,5V11A3,3 0 0,1 12,14A3,3 0 0,1 9,11V5A3,3 0 0,1 12,2Z"
            />
          </svg>
          UNIT 03: OPERATIONAL OVERRIDE
        </div>
        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px;">
          <button phx-click="start_scan" class="btn btn-primary" style="grid-column: span 2;">
            Execute Full Scan
          </button>
          <button phx-click="toggle_monitoring" class="btn">
            {if @monitoring_active, do: "Pause", else: "Resume"}
          </button>
          <button phx-click="purge_data" class="btn btn-danger">Purge DB</button>
        </div>
      </div>

      <!-- Unit 04: Traffic Guardian (Backpressure) -->
      <div class="card" style="border-color: var(--warning);">
        <div class="card-title" style="color: var(--warning);">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24">
            <path fill="currentColor" d="M12,2L1,21H23L12,2M12,6L19.53,19H4.47L12,6M11,10V14H13V10H11M11,16V18H13V16H11Z" />
          </svg>
          UNIT 04: TRAFFIC GUARDIAN
        </div>
        <div class="stat"><label>PRESSURE</label> <span style="color: var(--neon-green);">{@live.target_pressure} slots</span></div>
        <div class="stat">
          <label>T4_LATENCY</label>
          <span style={"color: #{if @live.t4_ema > 200, do: "var(--neon-red)", else: "var(--neon-blue)"};"}>
            {Float.round(@live.t4_ema, 2)}ms
          </span>
        </div>
        <div class="stat">
          <label>REAL_FLUX</label>
          <span style="color: var(--neon-blue);">{Float.round(@live.flux_reel, 1)} f/s</span>
        </div>
        
        <div class="progress-bar" style="background: rgba(217, 119, 6, 0.1);">
          <div class="progress-fill" style={"width: #{(@live.target_pressure / 1000) * 100}%; background: var(--warning);"}></div>
        </div>
      </div>
      
    <!-- Full Width: Real-time Activity Log -->
      <div class="card" style="grid-column: span 3; background: #000; border-color: #222;">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24">
            <path
              fill="currentColor"
              d="M13,9H11V7H13M13,17H11V11H13M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2Z"
            />
          </svg>
          REAL-TIME TELEMETRY STREAM
        </div>
        <div style="font-family: monospace; font-size: 0.75rem; height: 150px; overflow-y: hidden; display: flex; flex-direction: column-reverse;">
          <%= for file <- @live.last_files do %>
            <div style="padding: 4px 0; border-bottom: 1px solid #111; display: flex; gap: 15px;">
              <span style="color: var(--text-dim);">
                {String.slice(DateTime.to_iso8601(file.time), 11, 8)}
              </span>
              <span style={"color: #{if file.status == :ok, do: "var(--neon-green)", else: "var(--neon-red)"}"}>
                [{if file.status == :ok, do: "SUCCESS", else: "ERROR"}]
              </span>
              <span style="color: #fff;">{file.path}</span>
            </div>
          <% end %>
          <%= if Enum.empty?(@live.last_files) do %>
            <div style="color: var(--text-dim); text-align: center; margin-top: 60px;">
              SYSTEM_IDLE: AWAITING_INGESTION_DATA
            </div>
          <% end %>
          <div :if={Map.get(@live, :scan_complete, false)} style="color: var(--neon-green); text-align: center; margin-top: 12px;">
            Fleet Ingestion Complete
          </div>
        </div>
      </div>
      
    <!-- Knowledge Distribution Map -->
      <div class="card" style="grid-column: span 3;">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24">
            <path
              fill="currentColor"
              d="M20,18H4V8H20M20,6H12L10,4H4C2.89,4 2,4.89 2,6V18A2,2 0 0,0 4,20H20A2,2 0 0,0 22,18V8C22,6.89 21.1,6 20,6Z"
            />
          </svg>
          KNOWLEDGE DISTRIBUTION MAP (LIVE)
        </div>
        <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 15px;">
          <%= if @live.directories == %{} or is_nil(@live.directories) do %>
            <div
              class="stat"
              style="grid-column: span 3; text-align: center; border: 1px dashed var(--border); padding: 20px;"
            >
              WAITING FOR DATA... RUN FULL SCAN TO INITIALIZE TRACKER
            </div>
          <% else %>
            <%= for {dir, d_stats} <- @live.directories do %>
              <% percent =
                if d_stats.total > 0, do: trunc(d_stats.completed / d_stats.total * 100), else: 0 %>
              <div style="border-left: 2px solid var(--neon-blue); padding-left: 12px; background: rgba(0,242,255,0.02); padding-bottom: 8px;">
                <div style="display: flex; justify-content: space-between; align-items: baseline;">
                  <div style="font-size: 0.85rem; font-weight: 700; color: #fff; text-transform: uppercase;">
                    {dir}
                  </div>
                  <div style="font-size: 0.65rem; color: var(--text-dim);">
                    {if d_stats.last_update,
                      do: String.slice(DateTime.to_iso8601(d_stats.last_update), 11, 8),
                      else: "PENDING"}
                  </div>
                </div>

                <div style="display: flex; justify-content: space-between; margin-top: 8px; font-size: 0.7rem; font-family: monospace;">
                  <span style="color: var(--neon-green);">{d_stats.completed} DONE</span>
                  <span style="color: var(--neon-red);">
                    {if d_stats.failed > 0, do: "#{d_stats.failed} FAIL", else: ""}
                  </span>
                  <span style="color: var(--text-dim);">{d_stats.total} TOTAL</span>
                </div>

                <div class="progress-bar" style="height: 3px; margin-top: 5px;">
                  <div class="progress-fill" style={"width: #{percent}%"}></div>
                </div>
              </div>
            <% end %>
          <% end %>
        </div>
      </div>
    </div>
    """
  end

  defp apply_bridge_event(socket, %{"FileIndexed" => payload}) do
    path = Map.get(payload, "path", "unknown")
    status = if Map.get(payload, "status", "ok") == "ok", do: :ok, else: :error

    live =
      socket.assigns.live
      |> Map.update(:last_files, [%{path: path, status: status, time: DateTime.utc_now()}], fn files ->
        [%{path: path, status: status, time: DateTime.utc_now()} | Enum.take(files, 14)]
      end)
      |> Map.update(:total_ingested, 1, &(&1 + 1))
      |> Map.put(:scan_complete, false)

    assign(socket, live: live)
  end

  defp apply_bridge_event(socket, %{"ScanComplete" => _payload}) do
    live = Map.put(socket.assigns.live, :scan_complete, true)
    socket |> assign(live: live) |> put_flash(:info, "Fleet Ingestion Complete")
  end

  defp apply_bridge_event(socket, _event), do: socket
end
