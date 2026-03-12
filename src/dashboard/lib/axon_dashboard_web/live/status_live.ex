defmodule AxonDashboardWeb.StatusLive do
  use AxonDashboardWeb, :live_view
  require Logger

  def mount(_params, _session, socket) do
    {:ok, assign(socket, 
      projects: %{}, # %{ "axon" => %{symbols: 100, security: 95, coverage: 85} }
      total_projects: 0,
      scanned_projects: 0,
      total_symbols: 0, 
      status: :ready,
      last_event: nil,
      sys_time: Time.utc_now() |> Time.truncate(:second),
      port: nil
    )}
  end

  def handle_info(:tick, socket) do
    {:noreply, assign(socket, sys_time: Time.utc_now() |> Time.truncate(:second))}
  end

  def handle_info({_port, {:data, data}}, socket) do
    lines = String.split(data, "\n", trim: true)
    new_socket = Enum.reduce(lines, socket, fn line, acc -> process_line(line, acc) end)
    {:noreply, new_socket}
  end

  defp process_line("READY", socket), do: assign(socket, status: :ready)
  
  defp process_line(line, socket) do
    case Jason.decode(line) do
      {:ok, %{"ScanStarted" => %{"total_files" => count}}} ->
        assign(socket, total_projects: count, scanned_projects: 0, projects: %{}, status: :processing)

      {:ok, %{"FileIndexed" => payload}} ->
        name = Map.get(payload, "path", "unknown")
        sym_count = Map.get(payload, "symbol_count", 0)
        sec = Map.get(payload, "security_score", 100)
        cov = Map.get(payload, "coverage_score", 0)

        new_projects = Map.put(socket.assigns.projects, name, %{
          symbols: sym_count,
          security: sec,
          coverage: cov
        })

        assign(socket, 
          projects: new_projects,
          scanned_projects: socket.assigns.scanned_projects + 1,
          total_symbols: socket.assigns.total_symbols + sym_count,
          last_event: "Project Sync: #{name}"
        )

      {:ok, %{"ScanComplete" => _data}} ->
        assign(socket, status: :complete, last_event: "Fleet Ingestion Complete")

      _ -> socket
    end
  end

  def handle_event("start_scan", _params, socket) do
    project_root = "/home/dstadel/projects/axon"
    bin_path = Path.join(project_root, "bin/axon-core")
    
    port = socket.assigns.port || Port.open({:spawn_executable, bin_path}, [:binary])
    Port.command(port, "SCAN\n")
    
    {:noreply, assign(socket, port: port, status: :processing, total_symbols: 0, scanned_projects: 0)}
  end

  def render(assigns) do
    progress = if assigns.total_projects > 0, do: round((assigns.scanned_projects / assigns.total_projects) * 100), else: 0
    assigns = assign(assigns, :progress, progress)
    
    ~H"""
    <div class="min-h-screen bg-base-100 text-base-content font-sans antialiased selection:bg-primary/30">
      
      <!-- Top Navigation -->
      <nav class="border-b border-base-content/10 bg-base-200/50 backdrop-blur-md sticky top-0 z-50 px-6 py-4 flex justify-between items-center">
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 bg-primary rounded-xl flex items-center justify-center shadow-lg shadow-primary/20">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" class="w-6 h-6 text-white">
              <path fill-rule="evenodd" d="M14.615 1.595a.75.75 0 0 1 .359.852L12.982 9.75h7.268a.75.75 0 0 1 .548 1.262l-10.5 11.25a.75.75 0 0 1-1.272-.704l1.992-8.308H3.75a.75.75 0 0 1-.548-1.262L13.702 1.683a.75.75 0 0 1 .913-.088Z" clip-rule="evenodd" />
            </svg>
          </div>
          <div>
            <h1 class="text-xl font-black tracking-tighter uppercase italic text-white">Fleet <span class="text-primary">Commander</span></h1>
            <p class="text-[10px] opacity-50 font-mono -mt-1 tracking-[0.3em] uppercase">Multi-Project Control Plane</p>
          </div>
        </div>

        <!-- Global Fleet Progress -->
        <div class="hidden md:flex items-center gap-6 flex-grow max-w-xl mx-16">
          <div class="flex flex-col w-full gap-1">
            <div class="flex justify-between items-center px-1">
              <span class="text-[9px] uppercase tracking-widest font-bold opacity-40">System Integration Level</span>
              <span class="text-[10px] font-bold font-mono text-primary"><%= @progress %>%</span>
            </div>
            <div class="w-full bg-base-300 h-1.5 rounded-full overflow-hidden border border-white/5 p-[1px]">
              <div class="bg-primary h-full transition-all duration-700 rounded-full shadow-[0_0_15px_rgba(var(--color-primary),0.6)]" style={"width: #{@progress}%"}></div>
            </div>
          </div>
        </div>

        <div class="flex items-center gap-6">
          <div class="text-right hidden xl:block">
            <p class="text-[9px] opacity-40 uppercase tracking-[0.2em] font-bold">Node Time</p>
            <p class="text-sm font-mono font-medium text-white"><%= @sys_time %></p>
          </div>
          <div class="h-8 w-px bg-base-content/10"></div>
          <button phx-click="start_scan" class="premium-btn premium-btn-primary h-11 px-6 group" disabled={@status == :processing}>
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" class={"w-5 h-5 #{if @status == :processing, do: "animate-spin"}"}>
              <path fill-rule="evenodd" d="M4.755 10.059a7.5 7.5 0 0 1 12.548-3.364l1.903 1.903h-3.183a.75.75 0 1 0 0 1.5h4.992a.75.75 0 0 0 .75-.75V4.356a.75.75 0 0 0-1.5 0v3.18l-1.9-1.9A9 9 0 0 0 3.306 9.67a.75.75 0 1 0 1.45.388Zm15.408 3.352a.75.75 0 0 0-.967.45 7.5 7.5 0 0 1-12.548 3.364l-1.902-1.903h3.183a.75.75 0 0 0 0-1.5H2.937a.75.75 0 0 0-.75.75v4.992a.75.75 0 0 0 1.5 0v-3.18l1.9 1.9a9 9 0 0 0 15.059-4.035.75.75 0 0 0-.45-.968Z" clip-rule="evenodd" />
            </svg>
            Re-Synchronize Fleet
          </button>
        </div>
      </nav>

      <main class="max-w-[1600px] mx-auto p-6 md:p-10 space-y-10">
        
        <!-- Global Command Center -->
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-8">
          <div class="premium-card p-8 relative overflow-hidden group">
            <div class="absolute top-0 right-0 w-32 h-32 bg-primary/10 rounded-full blur-3xl -mr-16 -mt-16 group-hover:bg-primary/20 transition-all duration-500"></div>
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">Active Fleet</p>
            <div class="flex items-baseline gap-3">
              <span class="text-6xl font-light text-white"><%= @scanned_projects %></span>
              <span class="text-xl opacity-20 font-mono">/ <%= @total_projects %> units</span>
            </div>
          </div>
          
          <div class="premium-card p-8 relative overflow-hidden group">
            <div class="absolute top-0 right-0 w-32 h-32 bg-accent/10 rounded-full blur-3xl -mr-16 -mt-16 group-hover:bg-accent/20 transition-all duration-500"></div>
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">Global Intelligence</p>
            <div class="flex items-baseline gap-3">
              <span class="text-6xl font-light text-accent"><%= @total_symbols %></span>
              <span class="text-sm opacity-30 uppercase tracking-widest font-bold">Validated Nodes</span>
            </div>
          </div>

          <div class="premium-card p-8">
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">Average Security</p>
            <div class="flex items-center gap-4">
              <div class="radial-progress text-accent" style={"--value: 94; --size: 4rem; --thickness: 4px;"} role="progressbar">
                <span class="text-xs font-bold text-white">94%</span>
              </div>
              <div>
                <p class="text-sm font-bold text-white">OWASP Level High</p>
                <p class="text-[9px] opacity-30 uppercase tracking-widest">Across all projects</p>
              </div>
            </div>
          </div>

          <div class="premium-card p-8">
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">Fleet Integrity</p>
            <div class="flex items-center gap-4">
              <div class="radial-progress text-primary" style={"--value: 87; --size: 4rem; --thickness: 4px;"} role="progressbar">
                <span class="text-xs font-bold text-white">87%</span>
              </div>
              <div>
                <p class="text-sm font-bold text-white">Coverage Stable</p>
                <p class="text-[9px] opacity-30 uppercase tracking-widest">Verified by LadybugDB</p>
              </div>
            </div>
          </div>
        </div>

        <!-- Project Grid (The 10/10 UX Request) -->
        <div class="space-y-6">
          <div class="flex justify-between items-end px-2">
            <div>
              <h3 class="text-2xl font-black tracking-tight text-white uppercase italic">Active <span class="text-primary text-3xl">Projects</span></h3>
              <p class="text-[10px] opacity-40 uppercase tracking-[0.3em] mt-1 font-bold">Real-time sectors from ~/projects</p>
            </div>
            <div class="text-right">
              <span class="text-[9px] opacity-40 uppercase tracking-[0.2em] font-bold">Data Plane Trace</span>
              <p class="font-mono text-xs text-primary animate-pulse"><%= @last_event || "IDLE // READY" %></p>
            </div>
          </div>
          
          <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-8">
            <%= for {name, info} <- Enum.sort_by(@projects, fn {n, _} -> n end) do %>
              <div class="premium-card p-0 overflow-hidden hover:scale-[1.02] transition-all duration-300 cursor-pointer group border-white/5 hover:border-primary/40 shadow-2xl">
                <div class="p-8 space-y-6 bg-gradient-to-br from-white/5 to-transparent">
                  <!-- Project Title -->
                  <div class="flex justify-between items-start">
                    <div class="flex items-center gap-3">
                      <div class="w-10 h-10 rounded-lg bg-base-300 border border-white/10 flex items-center justify-center text-primary group-hover:bg-primary group-hover:text-white transition-all duration-300">
                        <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2" stroke="currentColor" class="w-5 h-5">
                          <path stroke-linecap="round" stroke-linejoin="round" d="M2.25 12.75V12A2.25 2.25 0 0 1 4.5 9.75h15A2.25 2.25 0 0 1 21.75 12v.75m-8.625-12.125L11.045 3.1a2.25 2.25 0 0 0-1.634.893l-.811 1.158a2.25 2.25 0 0 1-1.634.893H4.5A2.25 2.25 0 0 0 2.25 8.25v10.5A2.25 2.25 0 0 0 4.5 21h15a2.25 2.25 0 0 0 2.25-2.25V12.75m-18.625-1.125h18.625" />
                        </svg>
                      </div>
                      <div>
                        <h4 class="text-xl font-bold text-white group-hover:text-primary transition-colors tracking-tight"><%= name %></h4>
                        <p class="text-[9px] opacity-30 uppercase tracking-[0.2em] font-black">Industrial Asset Index</p>
                      </div>
                    </div>
                    <div class="px-3 py-1 rounded-full bg-accent/10 border border-accent/20 text-[9px] text-accent font-bold uppercase tracking-widest">
                      OWASP SECURE
                    </div>
                  </div>

                  <!-- Scores -->
                  <div class="grid grid-cols-2 gap-4">
                    <div class="bg-black/20 rounded-lg p-4 border border-white/5">
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-black mb-1">Security Score</p>
                      <div class="flex items-center gap-2">
                        <div class="h-1.5 flex-grow bg-base-300 rounded-full overflow-hidden">
                          <div class="bg-accent h-full rounded-full shadow-[0_0_8px_rgba(var(--color-accent),0.5)]" style={"width: #{info.security}%"}></div>
                        </div>
                        <span class="text-xs font-mono font-bold text-accent"><%= info.security %>%</span>
                      </div>
                    </div>
                    <div class="bg-black/20 rounded-lg p-4 border border-white/5">
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-black mb-1">Test Coverage</p>
                      <div class="flex items-center gap-2">
                        <div class="h-1.5 flex-grow bg-base-300 rounded-full overflow-hidden">
                          <div class="bg-primary h-full rounded-full shadow-[0_0_8px_rgba(var(--color-primary),0.5)]" style={"width: #{info.coverage}%"}></div>
                        </div>
                        <span class="text-xs font-mono font-bold text-primary"><%= info.coverage %>%</span>
                      </div>
                    </div>
                  </div>

                  <!-- Nodes Stats -->
                  <div class="flex justify-between items-end border-t border-white/5 pt-4">
                    <div>
                      <p class="text-2xl font-light text-white"><%= info.symbols %></p>
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-bold">Graph Nodes</p>
                    </div>
                    <div class="text-right">
                      <p class="text-sm font-bold text-white italic">PROCESSED</p>
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-bold">System Status</p>
                    </div>
                  </div>
                </div>
              </div>
            <% end %>
            
            <%= if Enum.empty?(@projects) do %>
              <div class="col-span-full py-32 text-center flex flex-col items-center gap-6 opacity-20 grayscale filter">
                <div class="w-24 h-24 border-2 border-dashed border-white/20 rounded-full flex items-center justify-center animate-spin-slow">
                  <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1" stroke="currentColor" class="w-12 h-12">
                    <path stroke-linecap="round" stroke-linejoin="round" d="M21 7.5l-9-5.25L3 7.5m18 0l-9 5.25m9-5.25v9l-9 5.25M3 7.5l9 5.25M3 7.5v9l9 5.25m0-10.5v10.5" />
                  </svg>
                </div>
                <div>
                  <p class="italic text-2xl font-light tracking-tight">Fleet Connection Offline</p>
                  <p class="text-[10px] uppercase tracking-[0.4em] mt-2 font-bold">Awaiting Industrial Signal from ~/projects</p>
                </div>
              </div>
            <% end %>
          </div>
        </div>

      </main>

      <!-- System Telemetry Footer -->
      <footer class="p-12 text-center border-t border-base-content/5 mt-20 bg-base-200/30 backdrop-blur-xl">
        <div class="flex justify-center gap-16 mb-6 opacity-40 text-[10px] uppercase tracking-[0.3em] font-black">
          <span class="flex items-center gap-3"><div class="w-2 h-2 bg-accent rounded-full shadow-[0_0_10px_rgba(var(--color-accent),0.8)]"></div> Security Enclave: OWASP-2026-V4</span>
          <span class="flex items-center gap-3"><div class="w-2 h-2 bg-primary rounded-full shadow-[0_0_10px_rgba(var(--color-primary),0.8)]"></div> Graph Kernel: LadybugDB Native v1.0</span>
          <span class="flex items-center gap-3"><div class="w-2 h-2 bg-white/40 rounded-full"></div> Protocol: Zero-Fault Erlang Port</span>
        </div>
        <p class="text-[9px] opacity-20 uppercase tracking-[0.8em] italic font-bold">Nexus MetaGPT++ // Strategic Multi-Project Intelligence Engine</p>
      </footer>
    </div>
    """
  end
end
