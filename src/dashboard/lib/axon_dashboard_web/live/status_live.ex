defmodule AxonDashboardWeb.StatusLive do
  use AxonDashboardWeb, :live_view
  require Logger

  def mount(_params, _session, socket) do
    socket =
      if connected?(socket) do
        :timer.send_interval(1000, self(), :tick)
        Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
        Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "telemetry_events")
        Phoenix.PubSub.subscribe(LiveView.Witness.PubSub, "witness_alerts")
        {:ok, _id, socket} = LiveView.Witness.expect_ui(socket, ".project-card", min_items: 1)
        {:ok, _id, socket} = LiveView.Witness.expect_ui(socket, "#resource-monitor", text: "Resource Intelligence")
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
        Axon.Watcher.Tracking.get_dashboard_stats()
      catch
        :exit, _ -> %{directories: %{}, last_files: []}
      end || %{directories: %{}, last_files: []}

    dirs = Map.get(stats, :directories, %{})
    last_f = Map.get(stats, :last_files, [])

    projects =
      Enum.reduce(dirs, %{}, fn {dir, info}, acc ->
        security = Map.get(state.security_scores, dir, 100)
        coverage = Map.get(state, :coverage_scores, %{}) |> Map.get(dir, 0)
        
        Map.put(acc, dir, %{
          symbols: info.symbols,
          relations: info.relations,
          files: info.completed + info.failed + info.ignored,
          entries: info.entries,
          security: security,
          coverage: coverage,
          total_files: info.total,
          failed_files: info.failed,
          ignored_files: info.ignored
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

  def handle_info(:tick, socket) do
    {:noreply,
     socket
     |> assign(sys_time: Time.utc_now() |> Time.truncate(:second))
     |> fetch_and_assign_stats()}
  end

  def handle_info(:trigger_initial_scan, socket) do
    AxonDashboard.BridgeClient.trigger_scan()

    {:noreply,
     assign(socket,
       status: :processing,
       total_symbols: 0,
       scanned_projects: 0,
       avg_security: 100,
       avg_coverage: 0
     )}
  end

  def handle_info({:telemetry_event, [:axon, :backpressure, :pressure_computed], measurements, metadata}, socket) do
    {:noreply, assign(socket, 
      system_pressure: measurements.pressure,
      cpu_load: metadata.cpu,
      ram_load: metadata.ram,
      io_wait: metadata.io
    )}
  end

  def handle_info({:telemetry_event, [:axon, :backpressure, :queues_paused], _measurements, _metadata}, socket) do
    {:noreply, assign(socket, queues_paused: true, indexing_limit: 0)}
  end

  def handle_info({:telemetry_event, [:axon, :backpressure, :queues_resumed], _measurements, _metadata}, socket) do
    {:noreply, assign(socket, queues_paused: false)}
  end

  def handle_info({:telemetry_event, [:axon, :backpressure, :limit_adjusted], measurements, _metadata}, socket) do
    {:noreply, assign(socket, indexing_limit: measurements.limit)}
  end

  def handle_info({:telemetry_event, [:axon, :watcher, :batch_enqueued], measurements, metadata}, socket) do
    msg = "[Watcher] Enqueued batch of #{measurements.count} files to #{metadata.queue}"
    {:noreply, assign(socket, last_event: msg)}
  end

  def handle_info({:telemetry_event, [:axon, :watcher, :batch_failed], _measurements, metadata}, socket) do
    alert = "ERROR: Failed to enqueue batch: #{metadata.error}"
    new_alerts = [alert | socket.assigns.alerts] |> Enum.take(3)
    {:noreply, assign(socket, alerts: new_alerts)}
  end

  def handle_info({:bridge_event, event}, socket) do
    new_socket = process_event(event, socket)
    {:noreply, new_socket}
  end

  def handle_info({:witness_alert, alert}, socket) do
    Logger.error("[LiveView.Witness] Critical alert received: #{inspect(alert)}")
    {:noreply, assign(socket, witness_alert: alert)}
  end

  def handle_info({:security_degraded, project, old, new}, socket) do
    alert = "CRITICAL: #{project} security dropped from #{old}% to #{new}%!"
    new_alerts = [alert | socket.assigns.alerts] |> Enum.take(3)
    {:noreply, assign(socket, alerts: new_alerts)}
  end

  def handle_info({:scan_started, _dir}, socket) do
    {:noreply, assign(socket, status: :processing, live_files: [])}
  end

  def handle_info({:file_indexed, path, status}, socket) do
    Logger.info("[LiveView] Received file_indexed: #{path} with status #{status}")
    {:noreply, socket}
  end

  defp process_event(%{"SystemReady" => %{"start_time_utc" => start_time}}, socket) do
    case DateTime.from_iso8601(start_time) do
      {:ok, dt, _offset} ->
        assign(socket, engine_start_time: dt)

      _ ->
        assign(socket, engine_start_time: nil)
    end
  end

  defp process_event(%{"ScanStarted" => %{"total_files" => count}}, socket) do
    assign(socket,
      total_projects: count,
      scanned_projects: 0,
      status: :processing,
      avg_security: 100,
      avg_coverage: 0
    )
  end

  defp process_event(
         %{"ProjectScanStarted" => %{"project" => name, "total_files" => total}},
         socket
       ) do
    assign(socket, last_event: "Project Started: #{name} [#{total} files]")
  end

  defp process_event(%{"FileIndexed" => payload}, socket) do
    name = Map.get(payload, "path", "unknown")
    assign(socket, last_event: "Indexing #{name}")
  end

  defp process_event(%{"ScanComplete" => _data}, socket) do
    assign(socket, status: :complete, last_event: "Fleet Ingestion Complete")
  end

  defp process_event(_, socket), do: socket

  def handle_event("start_scan", _params, socket) do
    AxonDashboard.BridgeClient.trigger_scan()

    {:noreply,
     assign(socket,
       status: :processing,
       total_symbols: 0,
       scanned_projects: 0,
       avg_security: 100,
       avg_coverage: 0
     )}
  end

  def handle_event("stop_scan", _params, socket) do
    AxonDashboard.BridgeClient.stop_scan()
    {:noreply, assign(socket, status: :ready, last_event: "Scan stopped by user.")}
  end

  def handle_event("dismiss_witness_alert", _params, socket) do
    {:noreply, assign(socket, witness_alert: nil)}
  end

  def handle_event("reset_db", _params, socket) do
    AxonDashboard.BridgeClient.reset_db()

    {:noreply,
     socket
     |> assign(
       status: :ready,
       avg_security: 100,
       avg_coverage: 0,
       last_event: "Database RESET."
     )
     |> fetch_and_assign_stats()}
  end

  def handle_event("phx-witness:certificate", report, socket) do
    LiveView.Witness.report_certificate(report)
    {:noreply, socket}
  end

  def handle_event("phx-witness:health_alert", alert, socket) do
    Logger.warning("[LiveView.Witness] Client-side health alert: #{inspect(alert)}")
    {:noreply, socket}
  end

  def render(assigns) do
    progress =
      if assigns.total_projects > 0,
        do: round(assigns.scanned_projects / assigns.total_projects * 100),
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
    <LiveView.Witness.HTML.witness_container id="witness-container" class="min-h-screen bg-base-100 text-base-content font-sans antialiased selection:bg-primary/30">
      
    <!-- Emergency Diagnostic Alert Overlay -->
      <%= if @witness_alert do %>
        <div class="fixed inset-0 z-[100] flex items-center justify-center bg-red-950/80 backdrop-blur-md p-6">
          <div class="max-w-2xl w-full bg-black border-2 border-red-500 rounded-3xl overflow-hidden shadow-[0_0_100px_rgba(239,68,68,0.5)] animate-in fade-in zoom-in duration-300">
            <div class="bg-red-600 p-8 flex items-center justify-between">
              <div class="flex items-center gap-4">
                <div class="p-3 bg-white/20 rounded-xl animate-pulse">
                  <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2.5" stroke="currentColor" class="w-8 h-8 text-white">
                    <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
                  </svg>
                </div>
                <div>
                  <h2 class="text-3xl font-black text-white uppercase italic tracking-tighter">EMERGENCY <span class="opacity-70">DIAGNOSTIC</span></h2>
                  <p class="text-white/60 text-xs font-mono uppercase tracking-widest">Out-of-Bound Critical Event Detected</p>
                </div>
              </div>
              <button phx-click="dismiss_witness_alert" class="p-2 hover:bg-white/20 rounded-full transition-colors">
                <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2" stroke="currentColor" class="w-6 h-6 text-white">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M6 18 18 6M6 6l12 12" />
                </svg>
              </button>
            </div>
            
            <div class="p-10 space-y-8">
              <div class="space-y-4">
                <div class="flex items-center justify-between text-[10px] font-black uppercase tracking-[0.3em] text-red-500/60">
                  <span>Source Signal</span>
                  <span class="font-mono">POD_A_ORACLE_DIRECT</span>
                </div>
                <div class="p-6 bg-red-500/5 border border-red-500/20 rounded-2xl font-mono text-lg text-red-400">
                  {Map.get(@witness_alert, "message") || Map.get(@witness_alert, "error") || inspect(@witness_alert)}
                </div>
              </div>

              <div class="grid grid-cols-2 gap-6">
                <div class="p-4 bg-white/5 rounded-xl border border-white/5">
                  <p class="text-[9px] uppercase tracking-widest opacity-40 font-bold mb-1">Timestamp</p>
                  <p class="text-white font-mono">{Time.utc_now() |> Time.truncate(:second) |> Time.to_string()}</p>
                </div>
                <div class="p-4 bg-white/5 rounded-xl border border-white/5">
                  <p class="text-[9px] uppercase tracking-widest opacity-40 font-bold mb-1">Status Code</p>
                  <p class="text-white font-mono">{Map.get(@witness_alert, "status") || "500 CRITICAL"}</p>
                </div>
              </div>

              <div class="flex gap-4">
                <button phx-click="dismiss_witness_alert" class="flex-grow bg-red-600 hover:bg-red-500 text-white font-black py-4 rounded-xl uppercase tracking-widest transition-all shadow-[0_10px_20px_rgba(220,38,38,0.3)]">
                  Acknowledge & Clear Signal
                </button>
                <button class="px-6 border-2 border-white/10 hover:border-white/30 text-white/60 hover:text-white rounded-xl transition-all">
                  <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-6 h-6">
                    <path stroke-linecap="round" stroke-linejoin="round" d="M3 16.5v2.25A2.25 2.25 0 0 0 5.25 21h13.5A2.25 2.25 0 0 0 21 18.75V16.5M16.5 12 12 16.5m0 0L7.5 12m4.5 4.5V3" />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>
      <% end %>

    <!-- Top Navigation -->
      <nav class="border-b border-base-content/10 bg-base-200/50 backdrop-blur-md sticky top-0 z-50 px-6 py-4 flex justify-between items-center">
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 bg-primary rounded-xl flex items-center justify-center shadow-lg shadow-primary/20">
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 24 24"
              fill="currentColor"
              class="w-6 h-6 text-white"
            >
              <path
                fill-rule="evenodd"
                d="M14.615 1.595a.75.75 0 0 1 .359.852L12.982 9.75h7.268a.75.75 0 0 1 .548 1.262l-10.5 11.25a.75.75 0 0 1-1.272-.704l1.992-8.308H3.75a.75.75 0 0 1-.548-1.262L13.702 1.683a.75.75 0 0 1 .913-.088Z"
                clip-rule="evenodd"
              />
            </svg>
          </div>
          <div>
            <h1 class="text-xl font-black tracking-tighter uppercase italic text-white">
              Fleet <span class="text-primary">Commander</span>
            </h1>
            <p class="text-[10px] opacity-50 font-mono -mt-1 tracking-[0.3em] uppercase">
              Multi-Project Control Plane
            </p>
          </div>
        </div>
        
    <!-- Global Fleet Progress -->
        <div class="hidden md:flex items-center gap-6 flex-grow max-w-xl mx-16">
          <div class="flex flex-col w-full gap-1">
            <div class="flex justify-between items-center px-1">
              <span class="text-[9px] uppercase tracking-widest font-bold opacity-40">
                System Integration Level
              </span>
              <span class="text-[10px] font-bold font-mono text-primary">{@progress}%</span>
            </div>
            <div class="w-full bg-base-300 h-1.5 rounded-full overflow-hidden border border-white/5 p-[1px]">
              <div
                class="bg-primary h-full transition-all duration-700 rounded-full shadow-[0_0_15px_rgba(var(--color-primary),0.6)]"
                style={"width: #{@progress}%"}
              >
              </div>
            </div>
          </div>
        </div>

        <div class="flex items-center gap-6">
          <div class="text-right hidden xl:block">
            <p class="text-[9px] opacity-40 uppercase tracking-[0.2em] font-bold">Engine Uptime</p>
            <p class="text-sm font-mono font-medium text-white">{@uptime_str}</p>
          </div>
          <div class="h-8 w-px bg-base-content/10"></div>

          <div class="flex gap-2">
            <button
              phx-click="start_scan"
              class="premium-btn premium-btn-primary h-11 px-6 group"
              disabled={@status == :processing}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
                fill="currentColor"
                class={"w-5 h-5 #{if @status == :processing, do: "animate-spin"}"}
              >
                <path
                  fill-rule="evenodd"
                  d="M4.755 10.059a7.5 7.5 0 0 1 12.548-3.364l1.903 1.903h-3.183a.75.75 0 1 0 0 1.5h4.992a.75.75 0 0 0 .75-.75V4.356a.75.75 0 0 0-1.5 0v3.18l-1.9-1.9A9 9 0 0 0 3.306 9.67a.75.75 0 1 0 1.45.388Zm15.408 3.352a.75.75 0 0 0-.967.45 7.5 7.5 0 0 1-12.548 3.364l-1.902-1.903h3.183a.75.75 0 0 0 0-1.5H2.937a.75.75 0 0 0-.75.75v4.992a.75.75 0 0 0 1.5 0v-3.18l1.9 1.9a9 9 0 0 0 15.059-4.035.75.75 0 0 0-.45-.968Z"
                  clip-rule="evenodd"
                />
              </svg>
              Start
            </button>

            <button
              phx-click="stop_scan"
              class="btn btn-outline border-white/20 text-white hover:bg-white/10 h-11 px-4"
              disabled={@status != :processing}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
                fill="currentColor"
                class="w-5 h-5 text-red-500"
              >
                <path
                  fill-rule="evenodd"
                  d="M4.5 7.5a3 3 0 0 1 3-3h9a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-9a3 3 0 0 1-3-3v-9Z"
                  clip-rule="evenodd"
                />
              </svg>
              Stop
            </button>

            <button
              phx-click="reset_db"
              class="btn btn-ghost hover:bg-red-500/20 hover:text-red-300 text-white/50 h-11 px-4"
              data-confirm="Are you sure you want to completely erase the graph database?"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                fill="none"
                viewBox="0 0 24 24"
                stroke-width="1.5"
                stroke="currentColor"
                class="w-5 h-5"
              >
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0"
                />
              </svg>
            </button>
          </div>
        </div>
      </nav>

      <%= if length(@alerts) > 0 do %>
        <div class="fixed top-24 right-6 z-50 flex flex-col gap-2">
          <%= for alert <- @alerts do %>
            <div class="bg-red-500/20 border border-red-500 text-red-100 px-6 py-4 rounded-xl shadow-[0_0_20px_rgba(239,68,68,0.3)] backdrop-blur-md animate-pulse">
              <div class="flex items-center gap-3">
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  class="h-6 w-6"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                >
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"
                  />
                </svg>
                <span class="font-bold text-sm tracking-wide uppercase">{alert}</span>
              </div>
            </div>
          <% end %>
        </div>
      <% end %>

      <main class="max-w-[1600px] mx-auto p-6 md:p-10 space-y-10">
        
    <!-- Global Command Center -->
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-8">
          <div class="premium-card p-8 relative overflow-hidden group">
            <div class="absolute top-0 right-0 w-32 h-32 bg-primary/10 rounded-full blur-3xl -mr-16 -mt-16 group-hover:bg-primary/20 transition-all duration-500">
            </div>
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">
              Active Fleet
            </p>
            <div class="flex items-baseline gap-3">
              <span class="text-6xl font-light text-white">{@scanned_projects}</span>
              <span class="text-xl opacity-20 font-mono">/ {@total_projects} units</span>
            </div>
          </div>

          <div class="premium-card p-8 relative overflow-hidden group">
            <div class="absolute top-0 right-0 w-32 h-32 bg-accent/10 rounded-full blur-3xl -mr-16 -mt-16 group-hover:bg-accent/20 transition-all duration-500">
            </div>
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">
              Global Intelligence
            </p>
            <div class="flex items-baseline gap-3">
              <span class="text-6xl font-light text-accent">{@total_symbols}</span>
              <span class="text-sm opacity-30 uppercase tracking-widest font-bold">
                Validated Nodes
              </span>
            </div>
          </div>

          <div class="premium-card p-8">
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">
              Average Security
            </p>
            <div class="flex items-center gap-4">
              <div
                class="radial-progress text-accent"
                style={"--value: #{@avg_security}; --size: 4rem; --thickness: 4px;"}
                role="progressbar"
              >
                <span class="text-xs font-bold text-white">{@avg_security}%</span>
              </div>
              <div>
                <p class="text-sm font-bold text-white">
                  OWASP Level {if @avg_security > 90, do: "High", else: "Medium"}
                </p>
                <p class="text-[9px] opacity-30 uppercase tracking-widest">Across all projects</p>
              </div>
            </div>
          </div>

          <div class="premium-card p-8">
            <p class="text-[10px] uppercase tracking-[0.3em] opacity-40 mb-2 font-black">
              Fleet Integrity
            </p>
            <div class="flex items-center gap-4">
              <div
                class="radial-progress text-primary"
                style={"--value: #{@avg_coverage}; --size: 4rem; --thickness: 4px;"}
                role="progressbar"
              >
                <span class="text-xs font-bold text-white">{@avg_coverage}%</span>
              </div>
              <div>
                <p class="text-sm font-bold text-white">
                  Coverage {if @avg_coverage > 80, do: "Stable", else: "Low"}
                </p>
                <p class="text-[9px] opacity-30 uppercase tracking-widest">Verified by LadybugDB</p>
              </div>
            </div>
          </div>
        </div>
        
    <!-- Resource Intelligence & Backpressure -->
        <div id="resource-monitor" class="premium-card p-8 bg-gradient-to-br from-base-200 to-base-300 border-primary/20 shadow-[0_0_40px_rgba(var(--color-primary),0.05)]">
          <div class="flex justify-between items-start mb-8">
            <div>
              <h3 class="text-lg font-black text-white uppercase italic tracking-tighter flex items-center gap-2">
                <div class={"w-2 h-2 rounded-full #{if @queues_paused, do: "bg-red-500 animate-pulse", else: "bg-green-500"}"}></div>
                Resource Intelligence <span class="text-primary opacity-50">// OS Monitor</span>
              </h3>
              <p class="text-[9px] opacity-40 uppercase tracking-widest font-bold mt-1">Real-time Backpressure & Oban Scaling</p>
            </div>
            <div class="text-right">
              <span class="text-[10px] font-mono text-primary font-bold">MODE: {if @queues_paused, do: "CONSTRAINED (PAUSED)", else: "DYNAMIC SCALING"}</span>
              <p class="text-[9px] opacity-30 uppercase tracking-widest mt-1">Worker Limit: {@indexing_limit} parallel jobs</p>
            </div>
          </div>

          <div class="grid grid-cols-1 md:grid-cols-4 gap-8">
            <div class="space-y-3">
              <div class="flex justify-between text-[10px] font-bold uppercase tracking-widest px-1">
                <span class="opacity-40">System Pressure</span>
                <span class={if @system_pressure >= 1.0, do: "text-red-500", else: "text-primary"}>
                  {Float.round(@system_pressure * 100, 1)}%
                </span>
              </div>
              <div class="h-2 bg-black/40 rounded-full overflow-hidden p-[1px] border border-white/5">
                <div class={"h-full rounded-full transition-all duration-500 #{if @system_pressure >= 1.0, do: "bg-red-500 shadow-[0_0_10px_rgba(239,68,68,0.5)]", else: "bg-primary shadow-[0_0_10px_rgba(var(--color-primary),0.5)]"}"} 
                     style={"width: #{min(@system_pressure * 100, 100)}%"}></div>
              </div>
            </div>

            <div class="space-y-3">
              <div class="flex justify-between text-[10px] font-bold uppercase tracking-widest px-1">
                <span class="opacity-40">CPU Load</span>
                <span class="text-white">{Float.round(@cpu_load, 1)}%</span>
              </div>
              <div class="h-2 bg-black/40 rounded-full overflow-hidden p-[1px] border border-white/5">
                <div class="h-full bg-white/20 rounded-full transition-all duration-500" style={"width: #{@cpu_load}%"}></div>
              </div>
            </div>

            <div class="space-y-3">
              <div class="flex justify-between text-[10px] font-bold uppercase tracking-widest px-1">
                <span class="opacity-40">RAM Usage</span>
                <span class="text-white">{Float.round(@ram_load, 1)}%</span>
              </div>
              <div class="h-2 bg-black/40 rounded-full overflow-hidden p-[1px] border border-white/5">
                <div class="h-full bg-white/20 rounded-full transition-all duration-500" style={"width: #{@ram_load}%"}></div>
              </div>
            </div>

            <div class="space-y-3">
              <div class="flex justify-between text-[10px] font-bold uppercase tracking-widest px-1">
                <span class="opacity-40">IO Wait</span>
                <span class="text-white">{Float.round(@io_wait, 1)}%</span>
              </div>
              <div class="h-2 bg-black/40 rounded-full overflow-hidden p-[1px] border border-white/5">
                <div class="h-full bg-white/20 rounded-full transition-all duration-500" style={"width: #{@io_wait}%"}></div>
              </div>
            </div>
          </div>
        </div>
        
    <!-- Semantic Error Visualization (Taint Analysis) -->
        <%= if map_size(@taint_paths) > 0 and Enum.any?(@taint_paths, fn {_, paths} -> length(paths) > 0 end) do %>
          <div class="premium-card p-8 border-red-500/30 bg-red-500/5">
            <h3 class="text-lg font-black text-white uppercase italic tracking-tighter flex items-center gap-2 mb-6">
              <div class="w-2 h-2 rounded-full bg-red-500 animate-ping"></div>
              Critical Semantic Violations <span class="text-red-500 opacity-50">// Taint Analysis Engine</span>
            </h3>
            
            <div class="space-y-6">
              <%= for {project, paths} <- @taint_paths, length(paths) > 0 do %>
                <div class="space-y-3">
                  <div class="flex items-center gap-2 text-xs font-bold text-red-400 uppercase tracking-widest">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2" stroke="currentColor" class="w-4 h-4">
                      <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m9-.75a9 9 0 1 1-18 0 9 9 0 0 1 18 0Zm-9 3.75h.008v.008H12v-.008Z" />
                    </svg>
                    Project: {project}
                  </div>
                  <div class="grid grid-cols-1 gap-3">
                    <%= for path <- Enum.take(paths, 3) do %>
                      <div class="bg-black/40 border border-white/5 p-4 rounded-xl font-mono text-[10px] space-y-2">
                        <div class="flex items-center gap-2 text-white/40">
                          <span class="px-2 py-0.5 bg-red-500/20 text-red-400 rounded-md font-bold">EXPOSURE PATH</span>
                        </div>
                        <div class="flex flex-wrap items-center gap-2">
                          <%= for node <- path["nodes"] do %>
                            <span class={"px-2 py-1 rounded border #{if node["properties"]["is_unsafe"] == "true", do: "border-red-500/50 text-red-400 bg-red-500/10", else: "border-white/10 text-white/60 bg-white/5"}"}>
                              {node["properties"]["name"] || "unknown"}
                            </span>
                            <%= if node != List.last(path["nodes"]) do %>
                              <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2" stroke="currentColor" class="w-3 h-3 text-white/20">
                                <path stroke-linecap="round" stroke-linejoin="round" d="M13.5 4.5 21 12m0 0-7.5 7.5M21 12H3" />
                              </svg>
                            <% end %>
                          <% end %>
                        </div>
                      </div>
                    <% end %>
                  </div>
                </div>
              <% end %>
            </div>
          </div>
        <% end %>

    <!-- Project Grid (The 10/10 UX Request) -->
        <div class="space-y-6">
          <div class="flex justify-between items-end px-2">
            <div>
              <h3 class="text-2xl font-black tracking-tight text-white uppercase italic">
                Active <span class="text-primary text-3xl">Projects</span>
              </h3>
              <p class="text-[10px] opacity-40 uppercase tracking-[0.3em] mt-1 font-bold">
                Real-time sectors from ~/projects
              </p>
            </div>
            <div class="text-right">
              <span class="text-[9px] opacity-40 uppercase tracking-[0.2em] font-bold">
                Data Plane Trace
              </span>
              <p class="font-mono text-xs text-primary animate-pulse">
                {@last_event || "IDLE // READY"}
              </p>
            </div>
          </div>

          <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-8">
            <%= for {name, info} <- Enum.sort_by(@projects, fn {n, _} -> n end) do %>
              <div class="project-card premium-card p-0 overflow-hidden hover:scale-[1.02] transition-all duration-300 cursor-pointer group border-white/5 hover:border-primary/40 shadow-2xl">
                <div class="p-8 space-y-6 bg-gradient-to-br from-white/5 to-transparent">
                  <!-- Project Title -->
                  <div class="flex justify-between items-start">
                    <div class="flex items-center gap-3">
                      <div class="w-10 h-10 rounded-lg bg-base-300 border border-white/10 flex items-center justify-center text-primary group-hover:bg-primary group-hover:text-white transition-all duration-300">
                        <svg
                          xmlns="http://www.w3.org/2000/svg"
                          fill="none"
                          viewBox="0 0 24 24"
                          stroke-width="2"
                          stroke="currentColor"
                          class="w-5 h-5"
                        >
                          <path
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            d="M2.25 12.75V12A2.25 2.25 0 0 1 4.5 9.75h15A2.25 2.25 0 0 1 21.75 12v.75m-8.625-12.125L11.045 3.1a2.25 2.25 0 0 0-1.634.893l-.811 1.158a2.25 2.25 0 0 1-1.634.893H4.5A2.25 2.25 0 0 0 2.25 8.25v10.5A2.25 2.25 0 0 0 4.5 21h15a2.25 2.25 0 0 0 2.25-2.25V12.75m-18.625-1.125h18.625"
                          />
                        </svg>
                      </div>
                      <div>
                        <h4 class="text-xl font-bold text-white group-hover:text-primary transition-colors tracking-tight">
                          {name}
                        </h4>
                        <p class="text-[9px] opacity-30 uppercase tracking-[0.2em] font-black">
                          Industrial Asset Index
                        </p>
                      </div>
                    </div>
                    <div class="px-3 py-1 rounded-full bg-accent/10 border border-accent/20 text-[9px] text-accent font-bold uppercase tracking-widest">
                      OWASP SECURE
                    </div>
                  </div>
                  
    <!-- Scores -->
                  <div class="grid grid-cols-2 gap-4">
                    <div class="bg-black/20 rounded-lg p-4 border border-white/5">
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-black mb-1">
                        Security Score
                      </p>
                      <div class="flex items-center gap-2">
                        <div class="h-1.5 flex-grow bg-base-300 rounded-full overflow-hidden">
                          <div
                            class="bg-accent h-full rounded-full shadow-[0_0_8px_rgba(var(--color-accent),0.5)]"
                            style={"width: #{info.security}%"}
                          >
                          </div>
                        </div>
                        <span class="text-xs font-mono font-bold text-accent">{info.security}%</span>
                      </div>
                    </div>
                    <div class="bg-black/20 rounded-lg p-4 border border-white/5">
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-black mb-1">
                        Test Coverage
                      </p>
                      <div class="flex items-center gap-2">
                        <div class="h-1.5 flex-grow bg-base-300 rounded-full overflow-hidden">
                          <div
                            class="bg-primary h-full rounded-full shadow-[0_0_8px_rgba(var(--color-primary),0.5)]"
                            style={"width: #{info.coverage}%"}
                          >
                          </div>
                        </div>
                        <span class="text-xs font-mono font-bold text-primary">{info.coverage}%</span>
                      </div>
                    </div>
                  </div>
                  
    <!-- Nodes Stats -->
                  <div class="flex justify-between items-end border-t border-white/5 pt-4">
                    <div class="flex gap-6">
                      <div>
                        <p class="text-2xl font-light text-white">{info.symbols}</p>
                        <p class="text-[9px] opacity-30 uppercase tracking-widest font-bold">Nodes</p>
                      </div>
                      <div>
                        <p class="text-2xl font-light text-accent">{info.relations}</p>
                        <p class="text-[9px] opacity-30 uppercase tracking-widest font-bold">
                          Relations
                        </p>
                      </div>
                      <div>
                        <p class="text-2xl font-light text-primary">
                          {info.files}<span class="text-sm opacity-50 font-mono">/<%= info.total_files %></span>
                        </p>
                        <p class="text-[9px] opacity-30 uppercase tracking-widest font-bold">Files</p>
                      </div>
                    </div>
                    <div class="text-right hidden xl:block">
                      <p class="text-sm font-bold text-white italic">PROCESSED</p>
                      <p class="text-[9px] opacity-30 uppercase tracking-widest font-bold">
                        System Status
                      </p>
                    </div>
                  </div>
                </div>
              </div>
            <% end %>

            <%= if Enum.empty?(@projects) do %>
              <div class="col-span-full py-32 text-center flex flex-col items-center gap-6 opacity-20 grayscale filter">
                <div class="w-24 h-24 border-2 border-dashed border-white/20 rounded-full flex items-center justify-center animate-spin-slow">
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke-width="1"
                    stroke="currentColor"
                    class="w-12 h-12"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      d="M21 7.5l-9-5.25L3 7.5m18 0l-9 5.25m9-5.25v9l-9 5.25M3 7.5l9 5.25M3 7.5v9l9 5.25m0-10.5v10.5"
                    />
                  </svg>
                </div>
                <div>
                  <p class="italic text-2xl font-light tracking-tight">
                    Fleet Online - Awaiting Projects
                  </p>
                  <p class="text-[10px] uppercase tracking-[0.4em] mt-2 font-bold">
                    Awaiting Industrial Signal from ~/projects
                  </p>
                </div>
              </div>
            <% end %>
          </div>
        </div>
        
    <!-- Matrix View: Live Ingestion Log -->
        <div class="mt-12 premium-card p-6 bg-black/80 border border-white/10">
          <div class="flex justify-between items-center mb-4">
            <h3 class="text-sm font-bold text-white uppercase tracking-widest flex items-center gap-2">
              <div class={"w-2 h-2 rounded-full #{if @cluster_connected, do: "bg-green-500 animate-pulse shadow-[0_0_8px_rgba(34,197,94,0.8)]", else: "bg-red-500"}"}>
              </div>
              Neural Link (Pod A Watcher)
            </h3>
            <span class="text-xs font-mono text-primary font-bold tracking-widest">
              {@total_files_parsed} FILES INGESTED
            </span>
          </div>
          <div class="h-64 overflow-y-auto font-mono text-xs space-y-1.5 flex flex-col-reverse p-2 bg-black rounded border border-white/5">
            <%= for {file, status} <- @live_files do %>
              <div class="flex items-center gap-4 py-1 border-b border-white/5 last:border-0">
                <span class="opacity-50 text-white">>_</span>
                <span class={"font-bold #{if status == :ok, do: "text-green-500", else: "text-red-500"}"}>
                  [{if status == :ok, do: "OK", else: "ERR"}]
                </span>
                <span class="text-green-400 opacity-80 truncate">{file}</span>
              </div>
            <% end %>
            <%= if Enum.empty?(@live_files) do %>
              <div class="text-white/30 italic py-4">Awaiting data stream from Watcher...</div>
            <% end %>
          </div>
        </div>
      </main>
      
    <!-- System Telemetry Footer -->
      <footer class="p-12 text-center border-t border-base-content/5 mt-20 bg-base-200/30 backdrop-blur-xl">
        <div class="flex justify-center gap-16 mb-6 opacity-40 text-[10px] uppercase tracking-[0.3em] font-black">
          <span class="flex items-center gap-3">
            <div class="w-2 h-2 bg-accent rounded-full shadow-[0_0_10px_rgba(var(--color-accent),0.8)]">
            </div>
            Security Enclave: OWASP-2026-V4
          </span>
          <span class="flex items-center gap-3">
            <div class="w-2 h-2 bg-primary rounded-full shadow-[0_0_10px_rgba(var(--color-primary),0.8)]">
            </div>
            Graph Kernel: LadybugDB Native v1.0
          </span>
          <span class="flex items-center gap-3">
            <div class="w-2 h-2 bg-white/40 rounded-full"></div>
            Protocol: Zero-Fault Erlang Port
          </span>
        </div>
        <p class="text-[9px] opacity-20 uppercase tracking-[0.8em] italic font-bold">
          Nexus MetaGPT++ // Strategic Multi-Project Intelligence Engine
        </p>
      </footer>
    </LiveView.Witness.HTML.witness_container>
    """
  end
end
