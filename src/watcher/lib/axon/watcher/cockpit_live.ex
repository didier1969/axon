defmodule Axon.Watcher.CockpitLive do
  use Phoenix.LiveView, layout: {Axon.Watcher.Layouts, :root}
  alias Axon.Watcher.Progress

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket), do: :timer.send_interval(1000, self(), :tick)
    
    repo_slug = System.get_env("AXON_REPO_SLUG") || (Path.expand(".") |> Path.basename())
    monitoring_active = Axon.Watcher.Server.get_monitoring_status()
    
    {:ok, assign(socket, 
      repo_slug: repo_slug, 
      stats: %{}, 
      dir_stats: %{}, 
      monitoring_active: monitoring_active
    )}
  end

  @impl true
  def handle_info(:tick, socket) do
    stats = Progress.get_status(socket.assigns.repo_slug)
    dir_stats = Progress.get_directory_stats(socket.assigns.repo_slug)
    monitoring_active = Axon.Watcher.Server.get_monitoring_status()
    {:noreply, assign(socket, stats: stats, dir_stats: dir_stats, monitoring_active: monitoring_active)}
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
      <div class="logo">AXON <span style="font-weight: 400; color: var(--text-dim);">SYSTEMS</span></div>
      <div style="display:flex; gap: 12px; align-items: center;">
        <div class={"status-badge #{if @monitoring_active, do: "status-live", else: "status-error"}"}>
          <%= if @monitoring_active, do: "● MONITORING ACTIVE", else: "○ MONITORING PAUSED" %>
        </div>
        <div class="pulse"></div>
      </div>
    </div>

    <div class="grid">
      <!-- Unit 01: Watcher -->
      <div class="card">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24"><path fill="currentColor" d="M12,9A3,3 0 0,0 9,12A3,3 0 0,0 12,15A3,3 0 0,0 15,12A3,3 0 0,0 12,9M12,17A5,5 0 0,1 7,12A5,5 0 0,1 12,7A5,5 0 0,1 17,12A5,5 0 0,1 12,17M12,4.5C7,4.5 2.73,7.61 1,12C2.73,16.39 7,19.5 12,19.5C17,19.5 21.27,16.39 23,12C21.27,7.61 17,4.5 12,4.5Z" /></svg>
          UNIT 01: CORE WATCHER
        </div>
        <div class="stat"><label>REPOSITORY</label> <span><%= @repo_slug %></span></div>
        <div class="stat"><label>STATUS</label> <span style="color: var(--neon-green);"><%= String.upcase(@stats["status"] || "live") %></span></div>
        <div class="stat"><label>LAST_SCAN</label> <span><%= String.slice(@stats["last_scan_at"] || "NEVER", 11, 8) %></span></div>
        
        <div class="progress-bar">
          <div class="progress-fill" style={"width: #{@stats["progress"] || 0}%"}></div>
        </div>
        <div style="display:flex; justify-content: space-between; margin-top: 10px; font-size: 0.7rem; font-weight: 700;">
          <span style="color: var(--text-dim);">INDEXING_LOAD</span>
          <span style="color: var(--neon-blue);"><%= @stats["progress"] || 0 %>%</span>
        </div>
      </div>

      <!-- Command Center -->
      <div class="card">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24"><path fill="currentColor" d="M12,15.5A2.5,2.5 0 0,1 14.5,18A2.5,2.5 0 0,1 12,20.5A2.5,2.5 0 0,1 9.5,18A2.5,2.5 0 0,1 12,15.5M12,2A3,3 0 0,1 15,5V11A3,3 0 0,1 12,14A3,3 0 0,1 9,11V5A3,3 0 0,1 12,2Z" /></svg>
          OPERATIONAL OVERRIDE
        </div>
        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px;">
          <button phx-click="start_scan" class="btn btn-primary" style="grid-column: span 2;">
            Execute Full Scan
          </button>
          
          <button phx-click="toggle_monitoring" class="btn">
            <%= if @monitoring_active, do: "Pause", else: "Resume" %>
          </button>

          <button phx-click="purge_data" data-confirm="DANGER: Purge database?" class="btn btn-danger">
            Purge DB
          </button>
        </div>
      </div>

      <!-- HydraDB Stats -->
      <div class="card">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24"><path fill="currentColor" d="M12,3C7.58,3 4,4.79 4,7C4,9.21 7.58,11 12,11C16.42,11 20,9.21 20,7C20,4.79 16.42,3 12,3M4,9V12C4,14.21 7.58,16 12,16C16.42,16 20,14.21 20,12V9C20,11.21 16.42,13 12,13C7.58,13 4,11.21 4,9M4,14V17C4,19.21 7.58,21 12,21C16.42,21 20,19.21 20,17V14C20,16.21 16.42,18 12,18C7.58,18 4,16.21 4,14Z" /></svg>
          UNIT 03: HYDRADB (POD C)
        </div>
        <div class="stat"><label>INGESTED_FILES</label> <span><%= @stats["synced"] || 0 %> / <%= @stats["total"] || 0 %></span></div>
        <div class="stat"><label>GRAPH_HEALTH</label> <span style="color: var(--neon-blue);">OPTIMAL</span></div>
        <div class="stat"><label>DATABASE_MODE</label> <span>INDUSTRIAL_TCP</span></div>
        <div class="stat" style="margin-top: 15px; font-size: 0.7rem; color: var(--text-dim);">
          SYSTEM_LAST_IMPORT: <%= String.slice(@stats["last_file_import_at"] || "N/A", 11, 8) %>
        </div>
      </div>

      <!-- Knowledge Coverage -->
      <div class="card" style="grid-column: span 3;">
        <div class="card-title">
          <svg style="width:18px;height:18px" viewBox="0 0 24 24"><path fill="currentColor" d="M20,18H4V8H20M20,6H12L10,4H4C2.89,4 2,4.89 2,6V18A2,2 0 0,0 4,20H20A2,2 0 0,0 22,18V8C22,6.89 21.1,6 20,6Z" /></svg>
          KNOWLEDGE DISTRIBUTION MAP
        </div>
        <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 15px;">
          <%= if Enum.empty?(@dir_stats) do %>
            <div class="stat" style="grid-column: span 3; text-align: center; border: 1px dashed var(--border); padding: 20px;">
              WAITING FOR DATA... RUN INITIAL SCAN
            </div>
          <% else %>
            <%= for {dir, count} <- @dir_stats do %>
              <div style="border-left: 2px solid var(--neon-blue); padding-left: 12px; background: rgba(0,242,255,0.02);">
                <div style="font-size: 0.65rem; color: var(--text-dim); text-transform: uppercase;"><%= dir %></div>
                <div style="font-size: 1.1rem; font-weight: 700; color: #fff; font-family: monospace;"><%= count %> <span style="font-size: 0.7rem; font-weight: 400; color: var(--text-dim);">FILES</span></div>
              </div>
            <% end %>
          <% end %>
        </div>
      </div>
    </div>
    """
  end
end
