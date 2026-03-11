defmodule AxonDashboardWeb.StatusLive do
  use AxonDashboardWeb, :live_view

  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
      :timer.send_interval(1000, self(), :tick)
    end

    {:ok, assign(socket, 
      events: [], 
      total_symbols: 0, 
      status: :waiting,
      last_file: "[ WAITING FOR SIGNAL ]",
      sys_time: DateTime.utc_now() |> DateTime.to_time() |> Time.truncate(:second)
    )}
  end

  def handle_info(:tick, socket) do
    {:noreply, assign(socket, sys_time: DateTime.utc_now() |> DateTime.to_time() |> Time.truncate(:second))}
  end

  def render(assigns) do
    ~H"""
    <div class="min-h-screen bg-[#050505] text-[#e0e0e0] p-4 md:p-8 relative overflow-hidden" phx-hook="TimeTick" id="hud-container">
      
      <!-- Top Navigation Bar / System Status -->
      <header class="flex justify-between items-center mb-8 border-b border-[#00ff41]/30 pb-4">
        <div class="flex items-center gap-4">
          <div class="w-3 h-3 bg-[#00ff41] rounded-full animate-pulse-glow"></div>
          <h1 class="text-3xl font-bold tracking-widest neon-text-green uppercase">AXON // V2_NEXUS</h1>
        </div>
        <div class="text-right">
          <div class="text-xs text-[#00ffff] opacity-70 uppercase tracking-widest">System Time</div>
          <div class="text-xl font-mono"><%= @sys_time %></div>
        </div>
      </header>

      <!-- Main HUD Grid -->
      <div class="grid grid-cols-1 lg:grid-cols-12 gap-6">
        
        <!-- Left Column: Metrics & Status -->
        <div class="lg:col-span-4 flex flex-col gap-6">
          
          <!-- Status Panel -->
          <div class="hud-panel p-6">
            <h2 class="text-xs text-[#00ffff] uppercase tracking-[0.2em] mb-4 border-b border-[#00ffff]/20 pb-2">Link Status</h2>
            
            <div class="flex items-center gap-4 mb-4">
              <div class={"w-16 h-16 rounded flex items-center justify-center border #{if @status == :processing, do: "border-[#00ff41] bg-[#00ff41]/10 text-[#00ff41]", else: "border-[#00ffff] bg-[#00ffff]/10 text-[#00ffff]"}"}>
                <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-8 h-8">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M8.25 3v1.5M4.5 8.25H3m18 0h-1.5M4.5 12H3m18 0h-1.5m-15 3.75H3m18 0h-1.5M8.25 19.5V21M12 3v1.5m0 15V21m3.75-18v1.5m0 15V21m-9-1.5h10.5a2.25 2.25 0 002.25-2.25V6.75a2.25 2.25 0 00-2.25-2.25H6.75A2.25 2.25 0 004.5 6.75v10.5a2.25 2.25 0 002.25 2.25z" />
                </svg>
              </div>
              <div>
                <div class="text-[10px] opacity-50 uppercase tracking-widest">UDS Socket</div>
                <div class={"text-2xl font-bold uppercase tracking-wider #{if @status == :processing, do: "neon-text-green animate-pulse-glow", else: "neon-text-cyan"}"}>
                  <%= if @status == :processing, do: "SYNCING...", else: Atom.to_string(@status) %>
                </div>
              </div>
            </div>

            <!-- Fake Security Metrics -->
            <div class="space-y-2 mt-6">
              <div class="flex justify-between text-xs">
                <span class="opacity-60">[SECURE ENCLAVE]</span>
                <span class="text-[#00ff41]">LOCKED</span>
              </div>
              <div class="flex justify-between text-xs">
                <span class="opacity-60">[GRAPH ENGINE]</span>
                <span class="text-[#00ffff]">LADYBUG_V1</span>
              </div>
              <div class="flex justify-between text-xs">
                <span class="opacity-60">[DATA PLANE]</span>
                <span class="text-[#00ff41]">RUST_NATIVE</span>
              </div>
            </div>
          </div>

          <!-- Telemetry Stats -->
          <div class="hud-panel p-6 flex-grow">
            <h2 class="text-xs text-[#00ffff] uppercase tracking-[0.2em] mb-4 border-b border-[#00ffff]/20 pb-2">Telemetry</h2>
            
            <div class="grid grid-cols-2 gap-4">
              <div class="bg-black/40 border border-[#1f1f1f] p-4 text-center">
                <div class="text-[10px] opacity-60 uppercase tracking-widest mb-1">Files Processed</div>
                <div class="text-3xl font-bold text-white"><%= length(@events) %></div>
              </div>
              <div class="bg-black/40 border border-[#1f1f1f] p-4 text-center">
                <div class="text-[10px] opacity-60 uppercase tracking-widest mb-1">Nodes Extracted</div>
                <div class="text-3xl font-bold neon-text-green"><%= @total_symbols %></div>
              </div>
            </div>

            <div class="mt-6">
              <div class="text-[10px] opacity-60 uppercase tracking-widest mb-2">Memory Allocation (Simulated)</div>
              <div class="w-full bg-[#111] h-2 rounded overflow-hidden">
                <div class="bg-[#00ff41] h-full" style="width: 34%"></div>
              </div>
              <div class="text-right text-[10px] mt-1 opacity-50">34% / 1024MB</div>
            </div>
          </div>
          
        </div>

        <!-- Right Column: Real-time Data Stream -->
        <div class="lg:col-span-8 flex flex-col gap-6">
          
          <!-- Current Target Scanner -->
          <div class="hud-panel p-4 border-[#00ffff]/40 neon-border-cyan">
            <div class="text-[10px] text-[#00ffff] uppercase tracking-[0.2em] mb-1">Current Target</div>
            <div class="font-mono text-sm truncate opacity-80 break-all">
              > <%= @last_file %>
            </div>
          </div>

          <!-- Data Stream Table -->
          <div class="hud-panel flex-grow flex flex-col overflow-hidden">
            <div class="p-4 border-b border-[#00ff41]/20 flex justify-between items-center bg-black/40">
              <h2 class="text-xs text-[#00ff41] uppercase tracking-[0.2em]">Live Ingestion Stream</h2>
              <div class="text-[10px] flex items-center gap-2">
                <span class="relative flex h-2 w-2">
                  <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-[#00ff41] opacity-75"></span>
                  <span class="relative inline-flex rounded-full h-2 w-2 bg-[#00ff41]"></span>
                </span>
                LIVE
              </div>
            </div>
            
            <div class="flex-grow overflow-auto p-4 max-h-[600px]">
              <table class="w-full text-left border-collapse">
                <thead>
                  <tr class="text-[10px] uppercase tracking-widest opacity-50 border-b border-[#333]">
                    <th class="pb-2 font-normal">Timestamp</th>
                    <th class="pb-2 font-normal">Path</th>
                    <th class="pb-2 font-normal text-right">Symbols</th>
                  </tr>
                </thead>
                <tbody class="font-mono text-xs">
                  <%= if Enum.empty?(@events) do %>
                    <tr>
                      <td colspan="3" class="py-8 text-center opacity-30 italic text-sm">
                        Waiting for data stream...
                      </td>
                    </tr>
                  <% else %>
                    <%= for {event, idx} <- Enum.with_index(Enum.take(@events, 50)) do %>
                      <tr class={"border-b border-[#111] hover:bg-[#00ff41]/10 transition-colors #{if idx == 0, do: "text-[#00ff41]"}"}>
                        <td class="py-2 opacity-60">
                          <%= event["ts"] || DateTime.utc_now() |> DateTime.to_time() |> Time.truncate(:second) %>
                        </td>
                        <td class="py-2 truncate max-w-[300px]" title={event["path"]}>
                          <%= event["path"] %>
                        </td>
                        <td class="py-2 text-right font-bold">
                          +<%= event["symbol_count"] %>
                        </td>
                      </tr>
                    <% end %>
                  <% end %>
                </tbody>
              </table>
            </div>
          </div>
          
        </div>
      </div>

    </div>
    """
  end

  def handle_info({:bridge_event, ["FileIndexed", data]}, socket) do
    # On ajoute un timestamp local si le rust ne l'envoie pas
    data_with_ts = Map.put(data, "ts", DateTime.utc_now() |> DateTime.to_time() |> Time.truncate(:second) |> Time.to_string())
    
    new_events = [data_with_ts | socket.assigns.events]
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
      last_file: "[ SCAN TERMINATED ]"
    )}
  end

  def handle_info({:bridge_event, _event}, socket) do
    {:noreply, socket}
  end
end
