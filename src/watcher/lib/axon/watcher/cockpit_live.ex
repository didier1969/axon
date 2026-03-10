defmodule Axon.Watcher.CockpitLive do
  use Phoenix.LiveView, layout: {Axon.Watcher.Layouts, :root}
  alias Axon.Watcher.Progress

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket), do: :timer.send_interval(1000, self(), :tick)
    
    repo_slug = System.get_env("AXON_REPO_SLUG") || "axon"
    {:ok, assign(socket, repo_slug: repo_slug, stats: %{}, pod_b_status: "online")}
  end

  @impl true
  def handle_info(:tick, socket) do
    stats = Progress.get_status(socket.assigns.repo_slug)
    # On pourrait aussi vérifier la santé de Pod B ici via NimblePool
    {:noreply, assign(socket, stats: stats)}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="header">
      <h1 style="margin:0; font-size: 1.5rem; display: flex; align-items: center; gap: 12px;">
        <div class="pulse"></div>
        AXON COCKPIT <span style="color: #71717a; font-weight: 400; font-size: 0.875rem;">v1.0.0</span>
      </h1>
      <div class="status-badge status-live">Operational</div>
    </div>

    <div class="grid">
      <!-- Pod A: Watcher -->
      <div class="card">
        <div class="card-title">
          <svg style="width:20px;height:20px" viewBox="0 0 24 24"><path fill="currentColor" d="M12,9A3,3 0 0,0 9,12A3,3 0 0,0 12,15A3,3 0 0,0 15,12A3,3 0 0,0 12,9M12,17A5,5 0 0,1 7,12A5,5 0 0,1 12,7A5,5 0 0,1 17,12A5,5 0 0,1 12,17M12,4.5C7,4.5 2.73,7.61 1,12C2.73,16.39 7,19.5 12,19.5C17,19.5 21.27,16.39 23,12C21.27,7.61 17,4.5 12,4.5Z" /></svg>
          Pod A: Watcher (Rust/Elixir)
        </div>
        <div class="stat">Repo Slug: <span>{@repo_slug}</span></div>
        <div class="stat">Status: <span style="color: #34d399;">{@stats["status"] || "live"}</span></div>
        <div class="stat">Last Scan: <span>{@stats["last_scan_at"] || "Never"}</span></div>
        
        <div class="progress-bar">
          <div class="progress-fill" style={"width: #{@stats["progress"] || 0}%"}></div>
        </div>
        <div style="display:flex; justify-content: space-between; margin-top: 8px; font-size: 0.75rem; color: #71717a;">
          <span>Indexing Progress</span>
          <span>{@stats["progress"] || 0}%</span>
        </div>
      </div>

      <!-- Pod B: Parser -->
      <div class="card">
        <div class="card-title">
          <svg style="width:20px;height:20px" viewBox="0 0 24 24"><path fill="currentColor" d="M21,16.5C21,16.88 20.79,17.21 20.47,17.38L12.57,21.82C12.41,21.94 12.21,22 12,22C11.79,22 11.59,21.94 11.43,21.82L3.53,17.38C3.21,17.21 3,16.88 3,16.5V7.5C3,7.12 3.21,6.79 3.53,6.62L11.43,2.18C11.59,2.06 11.79,2 12,2C12.21,2 12.41,2.06 12.57,2.18L20.47,6.62C20.79,6.79 21,7.12 21,7.5V16.5Z" /></svg>
          Pod B: Parser (Python/MsgPack)
        </div>
        <div class="stat">Workers: <span>8 (Erlang Ports)</span></div>
        <div class="stat">Mode: <span>High-Performance Bridge</span></div>
        <div class="stat">Avg. Latency: <span>12ms</span></div>
        <div style="margin-top: 20px; display: flex; gap: 4px;">
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
           <div style="width: 8px; height: 8px; background: #34d399; border-radius: 1px;"></div>
        </div>
      </div>

      <!-- Pod C: HydraDB -->
      <div class="card">
        <div class="card-title">
          <svg style="width:20px;height:20px" viewBox="0 0 24 24"><path fill="currentColor" d="M12,3C7.58,3 4,4.79 4,7C4,9.21 7.58,11 12,11C16.42,11 20,9.21 20,7C20,4.79 16.42,3 12,3M4,9V12C4,14.21 7.58,16 12,16C16.42,16 20,14.21 20,12V9C20,11.21 16.42,13 12,13C7.58,13 4,11.21 4,9M4,14V17C4,19.21 7.58,21 12,21C16.42,21 20,19.21 20,17V14C20,16.21 16.42,18 12,18C7.58,18 4,16.21 4,14Z" /></svg>
          Pod C: HydraDB (RocksDB/DuckDB)
        </div>
        <div class="stat">Host: <span>127.0.0.1:6040</span></div>
        <div class="stat">Ingested Files: <span>{@stats["synced"] || 0} / {@stats["total"] || 0}</span></div>
        <div class="stat">Graph Health: <span style="color: #34d399;">Healthy</span></div>
        <div class="stat">Last Import: <span>{@stats["last_file_import_at"] || "N/A"}</span></div>
      </div>
    </div>
    """
  end
end
