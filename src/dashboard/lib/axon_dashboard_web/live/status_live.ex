defmodule AxonDashboardWeb.StatusLive do
  use AxonDashboardWeb, :live_view
  require Logger

  def mount(_params, _session, socket) do
    socket =
      if connected?(socket) do
        :timer.send_interval(1000, self(), :tick_time)
        Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
        Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "telemetry_events")
        Phoenix.PubSub.subscribe(LiveView.Witness.PubSub, "witness_alerts")
        socket
      else
        socket
      end

    state =
      try do
        AxonDashboard.BridgeClient.get_state()
      catch
        :exit, _ -> %{}
      end

    start_time = Map.get(state, :engine_start_time)
    engine_state = Map.get(state, :engine_state, :idle)

    status = if engine_state == :indexing, do: :processing, else: :ready

    socket =
      socket
      |> assign(
        avg_security: 100,
        avg_coverage: 0,
        status: status,
        last_event: nil,
        sys_time: Time.utc_now() |> Time.truncate(:second),
        engine_start_time: start_time,
        alerts: [],
        witness_alert: nil,
        cluster_connected: true,
        # Resource Telemetry
        system_pressure: 0.0,
        cpu_load: 0.0,
        ram_load: 0.0,
        io_wait: 0.0,
        queues_paused: false,
        indexing_limit: 10,
        # Taint paths
        taint_paths: %{}
      )
      |> fetch_and_assign_stats()

    {:ok, socket}
  end

  defp fetch_and_assign_stats(socket) do
    state =
      try do
        AxonDashboard.BridgeClient.get_state()
      catch
        :exit, _ -> %{security_scores: %{}, taint_paths: %{}}
      end

    stats =
      try do
        Axon.Watcher.StatsCache.get_stats()
      catch
        :exit, _ -> %{projects: %{}, last_files: []}
      end || %{projects: %{}, last_files: []}

    dirs = Map.get(stats, :projects, %{})
    last_f = Map.get(stats, :last_files, [])

    projects =
      Enum.reduce(dirs, %{}, fn {dir, info}, acc ->
        progress =
          if info.total > 0,
            do: round((info.completed + info.failed + info.ignored) / info.total * 100),
            else: 0

        Map.put(acc, dir, %{
          symbols: info.symbols,
          relations: info.relations,
          files: info.completed + info.failed + info.ignored,
          entries: info.entries,
          security: info.security,
          coverage: info.coverage,
          total_files: info.total,
          failed_files: info.failed,
          ignored_files: info.ignored,
          progress: progress
        })
      end)

    live_files =
      Enum.map(last_f, fn f ->
        status_sym = if f.status == "indexed", do: :ok, else: :error
        {f.path, status_sym}
      end)

    total_parsed =
      Enum.reduce(dirs, 0, fn {_, info}, acc ->
        acc + info.completed + info.failed + info.ignored
      end)

    total_symbols = Enum.reduce(projects, 0, fn {_, p}, acc -> acc + p.symbols end)

    avg_security =
      if map_size(projects) > 0 do
        Enum.reduce(projects, 0, fn {_, p}, acc -> acc + p.security end) / map_size(projects)
      else
        100
      end

    avg_coverage =
      if map_size(projects) > 0 do
        Enum.reduce(projects, 0, fn {_, p}, acc -> acc + p.coverage end) / map_size(projects)
      else
        0
      end

    assign(socket,
      projects: projects,
      total_projects: map_size(projects),
      scanned_projects: map_size(projects),
      total_symbols: total_symbols,
      live_files: live_files,
      total_files_parsed: total_parsed,
      avg_security: round(avg_security),
      avg_coverage: round(avg_coverage),
      taint_paths: Map.get(state, :taint_paths, %{})
    )
  end

  def handle_info(:tick_time, socket) do
    {:noreply, assign(socket, sys_time: Time.utc_now() |> Time.truncate(:second))}
  end

  def handle_info(:stats_updated, socket) do
    {:noreply, fetch_and_assign_stats(socket)}
  end

  def handle_info(
        {:telemetry_event, [:axon, :backpressure, :pressure_computed], measurements, metadata},
        socket
      ) do
    {:noreply,
     assign(socket,
       system_pressure: measurements.pressure,
       cpu_load: metadata.cpu,
       ram_load: metadata.ram,
       io_wait: metadata.io
     )}
  end

  def handle_info(
        {:telemetry_event, [:axon, :backpressure, :queues_paused], _measurements, _metadata},
        socket
      ) do
    {:noreply, assign(socket, queues_paused: true, indexing_limit: 0)}
  end

  def handle_info(
        {:telemetry_event, [:axon, :backpressure, :queues_resumed], _measurements, _metadata},
        socket
      ) do
    {:noreply, assign(socket, queues_paused: false)}
  end

  def handle_info(
        {:telemetry_event, [:axon, :backpressure, :limit_adjusted], measurements, _metadata},
        socket
      ) do
    {:noreply, assign(socket, indexing_limit: measurements.limit)}
  end

  def handle_info(
        {:telemetry_event, [:axon, :watcher, :batch_enqueued], measurements, metadata},
        socket
      ) do
    msg = "[Watcher] Enqueued batch of #{measurements.count} files to #{metadata.queue}"
    {:noreply, assign(socket, last_event: msg)}
  end

  def handle_info(
        {:telemetry_event, [:axon, :watcher, :batch_failed], _measurements, metadata},
        socket
      ) do
    alert = "ERROR: Failed to enqueue batch: #{metadata.error}"
    new_alerts = [alert | socket.assigns.alerts] |> Enum.take(3)
    {:noreply, assign(socket, alerts: new_alerts)}
  end

  def handle_info({:bridge_event, event}, socket) do
    new_socket = process_event(event, socket)
    {:noreply, new_socket}
  end

  def handle_info({:witness_alert, alert}, socket) do
    {:noreply, assign(socket, witness_alert: alert)}
  end

  def handle_info({:security_degraded, project, old, new}, socket) do
    alert = "CRITICAL: #{project} security dropped from #{old}% to #{new}%!"
    new_alerts = [alert | socket.assigns.alerts] |> Enum.take(3)
    {:noreply, assign(socket, alerts: new_alerts)}
  end

  def handle_event("dismiss_witness_alert", _, socket) do
    {:noreply, assign(socket, witness_alert: nil)}
  end

  defp process_event({:file_indexed, _path}, socket), do: socket
  defp process_event({:project_scan_started, _proj, _total}, socket), do: socket
  defp process_event({:scan_complete, _total, _ms}, socket), do: assign(socket, status: :ready)
  defp process_event(_, socket), do: socket

  def render(assigns) do
    total_expected =
      Enum.reduce(assigns.projects, 0, fn {_, info}, acc -> acc + info.total_files end)

    progress =
      if total_expected > 0,
        do: round(assigns.total_files_parsed / total_expected * 100),
        else: 0

    assigns = assign(assigns, :progress, progress)

    uptime_str =
      if assigns.engine_start_time do
        diff = DateTime.diff(DateTime.utc_now(), assigns.engine_start_time, :second)
        mins = div(diff, 60)
        secs = rem(diff, 60)
        "#{mins}m #{secs}s"
      else
        "Booting..."
      end

    assigns = assign(assigns, :uptime_str, uptime_str)

    ~H"""
    <LiveView.Witness.HTML.witness_container
      id="witness-container"
      class="min-h-screen bg-slate-950 text-slate-200 font-mono antialiased p-4 md:p-8 flex flex-col gap-6"
    >
      
    <!-- EMERGENCY HUD OVERLAY -->
      <%= if @witness_alert do %>
        <div class="fixed inset-0 z-[100] flex items-center justify-center bg-slate-950/90 backdrop-blur-xl p-6">
          <div class="max-w-3xl w-full bg-slate-900 border-2 border-red-500 rounded-lg overflow-hidden shadow-[0_0_50px_rgba(239,68,68,0.3)]">
            <div class="bg-red-600/20 border-b border-red-500 p-6 flex items-center justify-between">
              <div class="flex items-center gap-4">
                <div class="w-3 h-3 bg-red-500 rounded-full animate-ping"></div>
                <h2 class="text-2xl font-black text-white tracking-tighter uppercase italic">
                  CRITICAL_SIGNAL_DETECTED
                </h2>
              </div>
              <button
                phx-click="dismiss_witness_alert"
                class="text-white/40 hover:text-white uppercase text-xs font-bold"
              >
                [ CLOSE ]
              </button>
            </div>
            <div class="p-8 space-y-6">
              <div class="bg-black/40 border border-red-500/20 p-6 rounded font-mono text-red-400">
                {Map.get(@witness_alert, "message") || Map.get(@witness_alert, "error") ||
                  inspect(@witness_alert)}
              </div>
              <div class="grid grid-cols-2 gap-4">
                <div class="bg-slate-800/50 p-4 border border-white/5">
                  <p class="text-[10px] text-slate-500 uppercase font-bold mb-1">Source Node</p>
                  <p class="text-white">AXON_CORE_V2_NATIVE</p>
                </div>
                <div class="bg-slate-800/50 p-4 border border-white/5">
                  <p class="text-[10px] text-slate-500 uppercase font-bold mb-1">Status</p>
                  <p class="text-red-400 font-bold">500_SYSTEM_FAULT</p>
                </div>
              </div>
              <button
                phx-click="dismiss_witness_alert"
                class="w-full bg-red-600 hover:bg-red-500 text-white font-black py-4 rounded uppercase tracking-widest transition-all"
              >
                Acknowledge & Flush Buffer
              </button>
            </div>
          </div>
        </div>
      <% end %>
      
    <!-- MAIN HUD TOP BAR (GRID COLS 12) -->
      <header class="grid grid-cols-12 gap-4 items-center">
        <div class="col-span-3 flex items-center gap-4 bg-slate-900/50 border border-white/5 p-4 rounded-lg">
          <div class="w-12 h-12 bg-amber-500 flex items-center justify-center rounded shadow-[0_0_20px_rgba(245,158,11,0.3)]">
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 24 24"
              fill="currentColor"
              class="w-8 h-8 text-black"
            >
              <path
                fill-rule="evenodd"
                d="M14.615 1.595a.75.75 0 0 1 .359.852L12.982 9.75h7.268a.75.75 0 0 1 .548 1.262l-10.5 11.25a.75.75 0 0 1-1.272-.704l1.992-8.308H3.75a.75.75 0 0 1-.548-1.262L13.702 1.683a.75.75 0 0 1 .913-.088Z"
                clip-rule="evenodd"
              />
            </svg>
          </div>
          <div>
            <h1 class="text-2xl font-black italic tracking-tighter text-white uppercase">
              AXON_<span class="text-amber-500">MAESTRIA</span>
            </h1>
            <p class="text-[10px] text-amber-500/50 font-bold uppercase tracking-widest">
              Tactical Control Plane v2.2
            </p>
          </div>
        </div>

        <div class="col-span-6 bg-slate-900/50 border border-white/5 p-4 rounded-lg flex flex-col gap-2">
          <div class="flex justify-between items-center text-[10px] font-black uppercase tracking-[0.2em]">
            <span class="text-slate-500">Global Fleet Sync</span>
            <span class="text-amber-500 font-mono">{@progress}% COMPLETED</span>
          </div>
          <div class="w-full bg-black/40 h-2 rounded-full overflow-hidden p-[1px] border border-white/5">
            <div
              class="bg-amber-500 h-full transition-all duration-1000 shadow-[0_0_15px_rgba(245,158,11,0.5)]"
              style={"width: #{@progress}%"}
            >
            </div>
          </div>
        </div>

        <div class="col-span-3 bg-slate-900/50 border border-white/5 p-4 rounded-lg flex justify-between items-center">
          <div class="text-left">
            <p class="text-[9px] text-slate-500 uppercase font-black tracking-widest">
              System Uptime
            </p>
            <p class="text-xl font-bold text-white tracking-tighter">{@uptime_str}</p>
          </div>
          <div class="text-right">
            <p class="text-[9px] text-slate-500 uppercase font-black tracking-widest">Kernel Time</p>
            <p class="text-xl font-bold text-white tracking-tighter">{@sys_time}</p>
          </div>
        </div>
      </header>
      
    <!-- SYSTEM INTELLIGENCE HUD -->
      <section class="grid grid-cols-12 gap-4">
        
    <!-- RESOURCE METRICS -->
        <div class="col-span-4 bg-slate-900/50 border border-white/5 rounded-lg p-6 flex flex-col gap-6">
          <h3 class="text-xs font-black text-slate-400 uppercase tracking-[0.3em] flex items-center gap-2">
            <div class={"w-2 h-2 rounded-full #{if @queues_paused, do: "bg-red-500 animate-pulse", else: "bg-emerald-500 animate-pulse"}"}>
            </div>
            System_Load_Monitor
          </h3>

          <div class="grid grid-cols-2 gap-6">
            <div class="space-y-2">
              <div class="flex justify-between text-[10px] font-bold text-slate-500">
                <span>CPU</span><span class="text-white">{@cpu_load}%</span>
              </div>
              <div class="h-1 bg-black/40 rounded-full overflow-hidden">
                <div class="bg-white/40 h-full" style={"width: #{@cpu_load}%"}></div>
              </div>
            </div>
            <div class="space-y-2">
              <div class="flex justify-between text-[10px] font-bold text-slate-500">
                <span>RAM</span><span class="text-white">{@ram_load}%</span>
              </div>
              <div class="h-1 bg-black/40 rounded-full overflow-hidden">
                <div class="bg-white/40 h-full" style={"width: #{@ram_load}%"}></div>
              </div>
            </div>
            <div class="space-y-2">
              <div class="flex justify-between text-[10px] font-bold text-slate-500">
                <span>IO_WAIT</span><span class="text-white">{@io_wait}%</span>
              </div>
              <div class="h-1 bg-black/40 rounded-full overflow-hidden">
                <div class="bg-white/40 h-full" style={"width: #{@io_wait}%"}></div>
              </div>
            </div>
            <div class="space-y-2">
              <div class="flex justify-between text-[10px] font-bold text-slate-500">
                <span>PRESSURE</span><span class="text-amber-500">{Float.round(@system_pressure * 100, 1)}%</span>
              </div>
              <div class="h-1 bg-black/40 rounded-full overflow-hidden">
                <div class="bg-amber-500 h-full" style={"width: #{min(@system_pressure * 100, 100)}%"}>
                </div>
              </div>
            </div>
          </div>

          <div class="mt-auto pt-4 border-t border-white/5 flex justify-between items-center text-[9px] font-bold uppercase tracking-widest text-slate-500">
            <span>Workers: {@indexing_limit} Parallel</span>
            <span class="text-amber-500">
              {if @queues_paused, do: "CONSTRAINED", else: "UNRESTRICTED"}
            </span>
          </div>
        </div>
        
    <!-- GLOBAL STATS -->
        <div class="col-span-8 grid grid-cols-3 gap-4">
          <div class="bg-slate-900/50 border border-white/5 rounded-lg p-6 flex flex-col justify-center">
            <p class="text-[10px] text-slate-500 uppercase font-black tracking-widest mb-2">
              Total Intelligence
            </p>
            <div class="flex items-baseline gap-2">
              <span class="text-5xl font-black text-white tracking-tighter italic">
                {@total_symbols}
              </span>
              <span class="text-xs text-slate-600 uppercase font-bold">Nodes</span>
            </div>
          </div>
          <div class="bg-slate-900/50 border border-white/5 rounded-lg p-6 flex flex-col justify-center">
            <p class="text-[10px] text-slate-500 uppercase font-black tracking-widest mb-2">
              Security Integrity
            </p>
            <div class="flex items-center gap-4">
              <span class="text-5xl font-black text-emerald-500 tracking-tighter italic">
                {@avg_security}%
              </span>
              <div class="text-[9px] text-slate-600 leading-tight font-bold uppercase">
                OWASP_V4<br />SECURE_STATE
              </div>
            </div>
          </div>
          <div class="bg-slate-900/50 border border-white/5 rounded-lg p-6 flex flex-col justify-center">
            <p class="text-[10px] text-slate-500 uppercase font-black tracking-widest mb-2">
              Test Reliability
            </p>
            <div class="flex items-center gap-4">
              <span class="text-5xl font-black text-amber-500 tracking-tighter italic">
                {@avg_coverage}%
              </span>
              <div class="text-[9px] text-slate-600 leading-tight font-bold uppercase">
                LADYBUG_DB<br />COVERAGE
              </div>
            </div>
          </div>
        </div>
      </section>
      
    <!-- PROJECT SECTORS GRID -->
      <section class="flex-grow flex flex-col gap-4">
        <div class="flex justify-between items-end px-2">
          <h2 class="text-xl font-black text-white tracking-tighter uppercase italic">
            Fleet_Sector_Map
            <span class="text-slate-600 opacity-50">// {map_size(@projects)} UNITS ONLINE</span>
          </h2>
          <div class="font-mono text-[10px] text-amber-500 animate-pulse">
            {@last_event || "IDLE_MONITORING_SYSTEM"}
          </div>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4">
          <%= for {name, info} <- Enum.sort_by(@projects, fn {_, info} -> info.total_files end, :desc) do %>
            <div class="bg-slate-900 border border-white/5 rounded overflow-hidden group hover:border-amber-500/50 transition-all duration-300 shadow-xl">
              <!-- Header -->
              <div class="bg-slate-800/50 p-4 border-b border-white/5 flex justify-between items-start">
                <div class="truncate pr-2">
                  <h4 class="text-white font-black uppercase tracking-tighter truncate">{name}</h4>
                  <p class="text-[9px] text-slate-500 font-bold uppercase">Sector_Asset</p>
                </div>
                <div class={"text-[10px] font-black #{if info.progress == 100, do: "text-emerald-500", else: "text-amber-500 animate-pulse"}"}>
                  {info.progress}%
                </div>
              </div>

              <div class="px-4 py-2 bg-slate-800/30 flex justify-end items-center border-b border-white/5">
                <div class="w-1.5 h-1.5 rounded-full bg-emerald-500 shadow-[0_0_5px_rgba(16,185,129,0.5)]">
                </div>
              </div>
              
    <!-- Metrics -->
              <div class="p-4 space-y-4">
                <div class="grid grid-cols-2 gap-2">
                  <div class="bg-black/40 p-2 rounded">
                    <p class="text-[8px] text-slate-600 uppercase font-black mb-1">Security</p>
                    <p class={"text-xs font-black #{if info.security > 90, do: "text-emerald-500", else: "text-amber-500"}"}>
                      {info.security}%
                    </p>
                  </div>
                  <div class="bg-black/40 p-2 rounded">
                    <p class="text-[8px] text-slate-600 uppercase font-black mb-1">Coverage</p>
                    <p class="text-xs text-white font-black">{info.coverage}%</p>
                  </div>
                </div>

                <div class="space-y-1">
                  <div class="flex justify-between text-[9px] font-bold text-slate-500 uppercase">
                    <span>Indexation</span>
                    <span class="text-slate-300">{info.files} / {info.total_files}</span>
                  </div>
                  <div class="h-1 bg-black/40 rounded-full overflow-hidden">
                    <div
                      class="bg-amber-500 h-full transition-all duration-500"
                      style={"width: #{info.progress}%"}
                    >
                    </div>
                  </div>
                </div>

                <div class="flex justify-between items-end text-[9px] font-bold text-slate-600 uppercase pt-2 border-t border-white/5">
                  <div>Nodes: <span class="text-white">{info.symbols}</span></div>
                  <div>Links: <span class="text-white">{info.relations}</span></div>
                </div>
              </div>
            </div>
          <% end %>
        </div>
      </section>
      
    <!-- NEURAL LINK TRACE (BOTTOM HUD) -->
      <footer class="mt-auto bg-black border border-white/10 rounded-lg p-4 h-48 flex flex-col">
        <div class="flex justify-between items-center mb-2 border-b border-white/5 pb-2">
          <div class="flex items-center gap-2">
            <div class="w-2 h-2 bg-emerald-500 rounded-full shadow-[0_0_8px_rgba(16,185,129,0.8)]">
            </div>
            <h3 class="text-[10px] font-black text-white uppercase tracking-[0.2em]">
              Neural_Link_Live_Trace
            </h3>
          </div>
          <div class="text-[9px] text-slate-600 font-bold uppercase tracking-widest">
            {assigns.total_files_parsed} FILES_STREAMED
          </div>
        </div>
        <div class="flex-grow overflow-y-auto font-mono text-[10px] space-y-1 scrollbar-hide flex flex-col-reverse">
          <%= for {file, status} <- @live_files do %>
            <div class="flex gap-4 opacity-70 hover:opacity-100 transition-opacity py-0.5">
              <span class="text-slate-700 font-bold">#SEQ_IDX</span>
              <span class={if status == :ok, do: "text-emerald-500", else: "text-red-500"}>
                [{if status == :ok, do: "SUCCESS", else: "FAILURE"}]
              </span>
              <span class="text-slate-400 truncate tracking-tight">{file}</span>
            </div>
          <% end %>
          <%= if Enum.empty?(@live_files) do %>
            <div class="text-slate-800 italic animate-pulse">Awaiting industrial data signal...</div>
          <% end %>
        </div>
      </footer>
    </LiveView.Witness.HTML.witness_container>
    """
  end
end
