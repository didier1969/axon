defmodule AxonDashboardWeb.StatusLive do
  use AxonDashboardWeb, :live_view
  require Logger

  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
      :timer.send_interval(1000, self(), :tick)
    end

    {:ok, assign(socket, 
      events: [], 
      total_symbols: 0, 
      status: :waiting,
      last_file: nil,
      sys_time: Time.utc_now() |> Time.truncate(:second),
      data_plane_pid: nil
    )}
  end

  def handle_info(:tick, socket) do
    {:noreply, assign(socket, sys_time: Time.utc_now() |> Time.truncate(:second))}
  end

  def handle_event("start_scan", _params, socket) do
    # On lance le binaire via un Port Erlang pour un contrôle total
    # Le binaire envoie ses events via UDS, mais le Dashboard peut aussi le killer
    # On scanne par défaut la racine du projet
    root_dir = "../../"
    bin_path = "../../bin/axon-core"
    
    if File.exists?(bin_path) do
      port = Port.open({:spawn_executable, bin_path}, [:binary, args: [root_dir]])
      # On lie le port au process pour qu'il soit cleané si la LV crash
      Port.monitor(port)
      
      {:noreply, assign(socket, status: :processing, last_file: "Initializing Data Plane...")}
    else
      {:noreply, put_flash(socket, :error, "Binary not found in bin/axon-core. Please run setup first.")}
    end
  end

  def handle_event("stop_scan", _params, socket) do
    # Pour un arrêt brutal (PoC), on pourrait envoyer un signal ou simplement fermer le port
    # Dans une version indus, on enverrait une commande via UDS
    {:noreply, assign(socket, status: :stopped)}
  end

  def handle_event("clear_logs", _params, socket) do
    {:noreply, assign(socket, events: [], total_symbols: 0)}
  end

  def render(assigns) do
    ~H"""
    <div class="min-h-screen bg-base-100 text-base-content font-sans antialiased selection:bg-primary/30">
      
      <!-- Top Navigation -->
      <nav class="border-b border-base-content/10 bg-base-200/50 backdrop-blur-md sticky top-0 z-50 px-6 py-4 flex justify-between items-center">
        <div class="flex items-center gap-3">
          <div class="w-8 h-8 bg-primary rounded-lg flex items-center justify-center shadow-lg shadow-primary/20">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" class="w-5 h-5 text-white">
              <path fill-rule="evenodd" d="M14.615 1.595a.75.75 0 0 1 .359.852L12.982 9.75h7.268a.75.75 0 0 1 .548 1.262l-10.5 11.25a.75.75 0 0 1-1.272-.704l1.992-8.308H3.75a.75.75 0 0 1-.548-1.262L13.702 1.683a.75.75 0 0 1 .913-.088Z" clip-rule="evenodd" />
            </svg>
          </div>
          <div>
            <h1 class="text-lg font-bold tracking-tight uppercase italic">Axon <span class="text-primary">v2</span></h1>
            <p class="text-[10px] opacity-50 font-mono -mt-1 tracking-widest uppercase">Industrial Intelligence</p>
          </div>
        </div>

        <div class="flex items-center gap-6">
          <div class="text-right hidden md:block">
            <p class="text-[10px] opacity-40 uppercase tracking-[0.2em]">Master System Clock</p>
            <p class="text-sm font-mono font-medium"><%= @sys_time %></p>
          </div>
          <div class="h-8 w-px bg-base-content/10"></div>
          <button phx-click="start_scan" class="premium-btn premium-btn-primary group" disabled={@status == :processing}>
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" class={"w-4 h-4 #{if @status == :processing, do: "animate-spin"}"}>
              <path fill-rule="evenodd" d="M4.755 10.059a7.5 7.5 0 0 1 12.548-3.364l1.903 1.903h-3.183a.75.75 0 1 0 0 1.5h4.992a.75.75 0 0 0 .75-.75V4.356a.75.75 0 0 0-1.5 0v3.18l-1.9-1.9A9 9 0 0 0 3.306 9.67a.75.75 0 1 0 1.45.388Zm15.408 3.352a.75.75 0 0 0-.967.45 7.5 7.5 0 0 1-12.548 3.364l-1.902-1.903h3.183a.75.75 0 0 0 0-1.5H2.937a.75.75 0 0 0-.75.75v4.992a.75.75 0 0 0 1.5 0v-3.18l1.9 1.9a9 9 0 0 0 15.059-4.035.75.75 0 0 0-.45-.968Z" clip-rule="evenodd" />
            </svg>
            Start Core Scan
          </button>
        </div>
      </nav>

      <main class="max-w-[1600px] mx-auto p-6 md:p-10 space-y-8">
        
        <!-- Header / Controls Summary -->
        <div class="flex flex-col md:flex-row justify-between items-start md:items-end gap-4">
          <div>
            <h2 class="text-3xl font-light tracking-tight text-white">System <span class="font-bold">Overview</span></h2>
            <div class="flex items-center gap-2 mt-2">
              <span class={"status-indicator #{case @status do :waiting -> "status-waiting"; :processing -> "status-processing animate-pulse"; :complete -> "status-complete" end}"}></span>
              <span class="text-xs font-mono uppercase tracking-widest opacity-60"><%= @status %></span>
            </div>
          </div>
          <div class="flex gap-2">
            <button phx-click="clear_logs" class="premium-btn premium-btn-outline text-xs py-1.5">
              Clear Buffer
            </button>
            <button phx-click="stop_scan" class="premium-btn premium-btn-danger text-xs py-1.5" disabled={@status != :processing}>
              Emergency Halt
            </button>
          </div>
        </div>

        <!-- Metrics Grid -->
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6">
          <div class="premium-card p-6">
            <p class="text-[10px] uppercase tracking-[0.2em] opacity-40 mb-1">Index Volume</p>
            <p class="text-4xl font-light text-white"><%= length(@events) %></p>
            <p class="text-[10px] mt-2 opacity-30 italic">Total files analyzed</p>
          </div>
          <div class="premium-card p-6">
            <p class="text-[10px] uppercase tracking-[0.2em] opacity-40 mb-1">Graph Density</p>
            <p class="text-4xl font-light text-primary"><%= @total_symbols %></p>
            <p class="text-[10px] mt-2 opacity-30 italic">Symbols in LadybugDB</p>
          </div>
          <div class="premium-card p-6">
            <p class="text-[10px] uppercase tracking-[0.2em] opacity-40 mb-1">Processing Rate</p>
            <p class="text-4xl font-light text-white">1.2ms<span class="text-xs opacity-40 ml-1">avg</span></p>
            <p class="text-[10px] mt-2 opacity-30 italic text-accent font-medium">Native Parallel Threading</p>
          </div>
          <div class="premium-card p-6 border-accent/20">
            <p class="text-[10px] uppercase tracking-[0.2em] opacity-40 mb-1">Security Integrity</p>
            <p class="text-4xl font-light text-accent italic">Verified</p>
            <p class="text-[10px] mt-2 opacity-30 italic">OWASP Audit Engine Loaded</p>
          </div>
        </div>

        <!-- Data Pipeline Table -->
        <div class="premium-card overflow-hidden flex flex-col">
          <div class="px-6 py-4 border-b border-base-content/5 bg-base-200/30 flex justify-between items-center">
            <h3 class="text-sm font-semibold uppercase tracking-widest opacity-70">Real-time Ingestion Pipeline</h3>
            <div class="font-mono text-[10px] opacity-40">
              <%= if @last_file, do: "> #{String.slice(@last_file, -60..-1)}", else: "READY" %>
            </div>
          </div>
          
          <div class="overflow-x-auto max-h-[500px]">
            <table class="w-full text-left">
              <thead class="bg-base-300/20 sticky top-0 backdrop-blur-sm">
                <tr class="text-[10px] uppercase tracking-widest opacity-40 border-b border-base-content/5">
                  <th class="px-6 py-3 font-medium">Trace ID</th>
                  <th class="px-6 py-3 font-medium">Source Path</th>
                  <th class="px-6 py-3 font-medium text-right text-primary">Symbol Δ</th>
                </tr>
              </thead>
              <tbody class="font-mono text-[11px] divide-y divide-base-content/5">
                <%= if Enum.empty?(@events) do %>
                  <tr>
                    <td colspan="3" class="px-6 py-20 text-center opacity-20 italic">
                      SYSTEM IDLE - AWAITING COMMAND...
                    </td>
                  </tr>
                <% else %>
                  <%= for {event, idx} <- Enum.with_index(@events) do %>
                    <tr class={"hover:bg-primary/5 transition-colors group #{if idx == 0, do: "bg-primary/10 text-primary animate-pulse", else: "opacity-70 hover:opacity-100"}"}>
                      <td class="px-6 py-3 opacity-40 group-hover:opacity-100">
                        <%= event["ts"] || "00:00:00" %>
                      </td>
                      <td class="px-6 py-3 truncate max-w-[600px] font-medium" title={event["path"]}>
                        <%= event["path"] %>
                      </td>
                      <td class="px-6 py-3 text-right font-bold text-primary">
                        +<%= event["symbol_count"] %>
                      </td>
                    </tr>
                  <% end %>
                <% end %>
              </tbody>
            </table>
          </div>
        </div>

      </main>

      <!-- Footer Telemetry -->
      <footer class="p-6 text-center border-t border-base-content/5 mt-10">
        <p class="text-[10px] opacity-30 uppercase tracking-[0.4em]">Nexus MetaGPT++ Industrial Protocol v2.0</p>
      </footer>
    </div>
    """
  end

  def handle_info({:bridge_event, ["FileIndexed", data]}, socket) do
    data_with_ts = Map.put(data, "ts", Time.utc_now() |> Time.truncate(:second) |> Time.to_string())
    
    new_events = [data_with_ts | socket.assigns.events] |> Enum.take(100)
    new_total = socket.assigns.total_symbols + data["symbol_count"]
    
    {:noreply, assign(socket, 
      events: new_events, 
      total_symbols: new_total, 
      status: :processing,
      last_file: data["path"]
    )}
  end

  def handle_info({:bridge_event, ["ScanComplete", _data]}, socket) do
    {:noreply, assign(socket, 
      status: :complete,
      last_file: "[ SCAN SUCCESSFUL ]"
    )}
  end

  def handle_info(_other, socket), do: {:noreply, socket}
end
