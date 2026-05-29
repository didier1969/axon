defmodule AxonDashboardWeb.Live.PipelineLive do
  @moduledoc """
  REQ-AXO-901647 page 1 — Pipeline V2 streaming cockpit.

  Renders the canonical CPT-AXO-054 topology

      A1 read+hash → A2 parse-TS → A3 graph-UPSERT
        ─try_send (A3→B1 cap 10000)──▶
      B1 fetch    → B2 embed-GPU → B3 write-emb

  REQ-AXO-901806 / REQ-AXO-901826 — single source of truth =
  `%AxonDashboard.DashboardState{}` struct broadcast on the dedicated
  PubSub topic `BridgeClient.dashboard_topic/0` at 1 Hz. No more
  string-key map access, no more shared `bridge_events` topic
  noise (which carries FileIndexed / ScanStarted etc.).
  """
  use Phoenix.LiveView

  alias AxonDashboard.BridgeClient
  alias AxonDashboard.DashboardState
  alias AxonDashboardWeb.Live.Nav

  @rate_window_size 60
  @push_min_interval_ms 250

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, BridgeClient.dashboard_topic())
    end

    initial = BridgeClient.dashboard_state() || %DashboardState{}

    socket =
      socket
      |> assign(:page_title, "Axon · Pipeline Cockpit")
      |> assign(:dashboard_state, initial)
      |> assign(:rate_series, :queue.new())
      |> assign(:last_push_ms, 0)
      |> stream(:per_project, initial.per_project, dom_id: &"proj-#{&1.project_code}")

    {:ok, push_pipeline_state(socket, force: true)}
  end

  @impl true
  def handle_info({:dashboard_state, %DashboardState{} = state}, socket) do
    socket =
      socket
      |> assign(:dashboard_state, state)
      |> update_rate_series(state)
      |> maybe_push_pipeline_state()
      |> stream(:per_project, state.per_project, reset: true, dom_id: &"proj-#{&1.project_code}")

    {:noreply, socket}
  end

  # Catch-all keeps process alive on any other broadcast.
  def handle_info(_msg, socket), do: {:noreply, socket}

  @impl true
  def render(assigns) do
    ~H"""
    <Nav.shell
      current={:pipeline}
      build_id={runtime_field(@dashboard_state, :build_id, "n/a")}
      install_generation={runtime_field(@dashboard_state, :install_generation, "n/a")}
      runtime_mode={runtime_field(@dashboard_state, :runtime_mode, "unknown")}
      instance_kind={runtime_field(@dashboard_state, :instance_kind, "unknown")}
      gpu_effective={embedder_field(@dashboard_state, :effective, "unknown")}
      degraded_reason={runtime_field(@dashboard_state, :degraded_reason, nil)}
      stale={is_nil(@dashboard_state.ts_ms)}
      observed_age_ms={DashboardState.observed_age_ms(@dashboard_state)}
    >
      <div class="grid grid-cols-12 gap-4">
        <%!-- REQ-AXO-901827 — bandeau validation conditionnelle.
             Tant que l'indexer sous-indexe (FS watcher fd exhaustion +
             tree-sitter parsers manquants), les valeurs PG sont une
             vérité PARTIELLE. Le dashboard reflète fidèlement ce que
             PG retourne — mais PG retourne lui-même une valeur fausse
             car symbols/chunks/embedded incomplets. À retirer une
             fois REQ-AXO-901827 fermée. --%>
        <section class="col-span-12 rounded-xl border border-red-500/40 bg-red-950/30 px-5 py-3">
          <div class="flex items-center gap-3">
            <span class="h-2.5 w-2.5 rounded-full bg-red-400 animate-pulse"></span>
            <div class="flex-1">
              <div class="text-[10px] uppercase tracking-[0.18em] text-red-300 font-semibold">
                Valeurs non validées — REQ-AXO-901827
              </div>
              <div class="mt-1 text-[12px] text-red-200/90 leading-snug">
                L'indexer dev sous-indexe (FS watcher : <code class="bg-red-950/70 px-1 rounded">Too many open files (os 24)</code> + tree-sitter parsers manquants
                pour JSON/TOML/lock). Symptômes : 17 projets sur 25 à 0 symbols, ratio symbols/file = 0.52 (attendu 10-30).
                Les chiffres ci-dessous sont la vérité PG actuelle, mais cette vérité est incomplète.
                Ne pas valider comme final tant que le ratio symbols/file n'est pas dans la norme.
              </div>
            </div>
          </div>
        </section>

        <%!-- INDEXATION FUNNEL --%>
        <section class="col-span-12 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm px-5 py-3">
          <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80 mb-2">Indexation Funnel</div>
          <div class="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-sm text-slate-200">
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Disk</span>
              <strong class="tabular-nums">{fs_val(@dashboard_state, :disk_files)}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Eligible</span>
              <strong class="tabular-nums">{fs_val(@dashboard_state, :eligible_files)}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Indexed</span>
              <strong class="tabular-nums">{totals_field(@dashboard_state, :files, 0) |> full_int()}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Chunks</span>
              <strong class="tabular-nums">{totals_field(@dashboard_state, :chunks, 0) |> full_int()}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Embeddings</span>
              <strong class={["tabular-nums", coverage_text_class(totals_field(@dashboard_state, :coverage_pct, 0.0))]}>
                {totals_field(@dashboard_state, :embedded, 0) |> full_int()}
              </strong>
              <span class="text-slate-500 text-[10px] ml-1">
                ({:erlang.float_to_binary(totals_field(@dashboard_state, :coverage_pct, 0.0) * 1.0, decimals: 1)}%)
              </span>
            </span>
          </div>
        </section>

        <%!-- COVERAGE SUMMARY --%>
        <section class="col-span-12 grid grid-cols-2 md:grid-cols-4 xl:grid-cols-6 gap-3">
          <.kpi label="Indexed Files" value={totals_field(@dashboard_state, :files, 0) |> full_int()} tone={:neutral} />
          <.kpi label="Symbols" value={totals_field(@dashboard_state, :symbols, 0) |> full_int()} tone={:neutral} />
          <.kpi label="Edges" value={totals_field(@dashboard_state, :edges, 0) |> full_int()} tone={:neutral} />
          <.kpi
            label="Total Chunks"
            value={totals_field(@dashboard_state, :chunks, 0) |> full_int()}
            tone={:neutral}
          />
          <.kpi
            label="Embedded"
            value={totals_field(@dashboard_state, :embedded, 0) |> full_int()}
            sub={"#{:erlang.float_to_binary(totals_field(@dashboard_state, :coverage_pct, 0.0) * 1.0, decimals: 2)}%"}
            tone={coverage_tone(totals_field(@dashboard_state, :coverage_pct, 0.0))}
          />
          <.kpi
            label="Pending"
            value={totals_field(@dashboard_state, :pending, 0) |> full_int()}
            sub={"queue: #{totals_field(@dashboard_state, :pending, 0) |> full_int()}"}
            tone={pending_tone(@dashboard_state)}
          />
        </section>

        <%!-- PIPELINE V2 SVG TOPOLOGY --%>
        <section class="col-span-12 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm overflow-hidden">
          <header class="flex items-center justify-between px-5 py-3 border-b border-slate-800 bg-slate-950/50">
            <div>
              <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Pipeline V2 · CPT-AXO-054</div>
              <h2 class="text-base font-semibold text-slate-100 mt-0.5">A1/A2/A3 → try_send → B1/B2/B3</h2>
            </div>
            <div class="flex items-center gap-3 text-[11px] font-mono">
              <span class="flex items-center gap-1.5 text-slate-300">
                <span class="h-1.5 w-1.5 rounded-full bg-emerald-400 animate-pulse"></span>
                live · 1Hz
              </span>
              <span class="text-slate-500">
                lifecycle: <span class="text-slate-200">{lifecycle_field(@dashboard_state, :phase, "?")}</span>
              </span>
            </div>
          </header>
          <div
            id="pipeline-topology"
            phx-hook="PipelineTopology"
            phx-update="ignore"
            class="w-full"
            style="height: 320px;"
          ></div>
          <div class="px-5 py-3 border-t border-slate-800 bg-slate-950/40 flex flex-wrap items-center gap-4 text-[10px] font-mono uppercase tracking-wider text-slate-500">
            <span>A3→B1 buffer cap: <strong class="text-slate-200">{pipeline_field(@dashboard_state, :a3_to_b1_buffer_cap, 0) |> full_int()}</strong></span>
            <span>NOTIFY: <strong class="text-slate-200">{rc_field(@dashboard_state, :notify_channel, "n/a")}</strong></span>
            <span>coldstart poll: <strong class="text-slate-200">{rc_field(@dashboard_state, :coldstart_poll_interval_secs, 0)}s × {pipeline_field(@dashboard_state, :coldstart_batch_size, 0) |> full_int()}</strong></span>
            <span>idle: <strong class={if runtime_field(@dashboard_state, :runtime_idle, false), do: "text-emerald-300", else: "text-amber-300"}>{runtime_field(@dashboard_state, :runtime_idle, false) |> to_string()}</strong></span>
          </div>
        </section>

        <%!-- LIVE RATE PANEL --%>
        <section class="col-span-12 lg:col-span-7 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="flex items-center justify-between px-5 py-3 border-b border-slate-800">
            <div>
              <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Live Throughput</div>
              <h2 class="text-base font-semibold text-slate-100 mt-0.5">B2 embedder rate (chunks/sec)</h2>
            </div>
            <div class="text-right">
              <div class="font-mono text-2xl font-semibold tabular-nums text-emerald-300">
                {format_float(telemetry_field(@dashboard_state, :chunk_embeddings_per_second, 0.0))}
              </div>
              <div class="text-[10px] uppercase tracking-wider text-slate-500">chunks/s (5s window)</div>
            </div>
          </header>
          <div class="p-4">
            <div class="flex items-end gap-[2px] h-24" id="rate-sparkline">
              <%= for {h, idx} <- Enum.with_index(rate_bar_heights(@rate_series)) do %>
                <div
                  class="flex-1 bg-gradient-to-t from-emerald-600/30 to-emerald-400 rounded-sm transition-all duration-300"
                  style={"height: #{h}%"}
                  data-idx={idx}
                ></div>
              <% end %>
            </div>
            <div class="mt-3 flex items-center justify-between text-[10px] font-mono uppercase tracking-wider text-slate-500">
              <span>chunks embedded (Σ): <strong class="text-slate-200">{telemetry_field(@dashboard_state, :vector_chunks_embedded_total, 0) |> full_int()}</strong></span>
              <span>scheduler: <strong class="text-slate-200">{telemetry_field(@dashboard_state, :scheduler, "?")}</strong></span>
              <span>pressure: <strong class={pressure_class(telemetry_field(@dashboard_state, :service_pressure, "?"))}>{telemetry_field(@dashboard_state, :service_pressure, "?")}</strong></span>
            </div>
          </div>
        </section>

        <%!-- GPU / EMBEDDER PROVIDER PANEL --%>
        <section class="col-span-12 lg:col-span-5 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="flex items-center justify-between px-5 py-3 border-b border-slate-800">
            <div>
              <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">B2 Embedder</div>
              <h2 class="text-base font-semibold text-slate-100 mt-0.5">ONNX Runtime · BGE-Large 1024d</h2>
            </div>
            <span class={[
              "px-2 py-1 rounded-md text-[10px] font-mono uppercase tracking-wide border",
              embedder_class(embedder_field(@dashboard_state, :effective, "unknown"))
            ]}>
              {embedder_field(@dashboard_state, :effective, "unknown")}
            </span>
          </header>
          <div class="p-4 space-y-3">
            <.kv label="Requested provider" value={embedder_field(@dashboard_state, :requested, "n/a")} />
            <.kv label="Effective provider" value={embedder_field(@dashboard_state, :effective, "n/a")} />
            <.kv label="Init error" value={embedder_field(@dashboard_state, :init_error, nil) || "—"} />
            <.kv label="B2 batch" value={"#{pipeline_field(@dashboard_state, :b2_batch_size, 0)} chunks / #{pipeline_field(@dashboard_state, :b2_batch_timeout_ms, 0)} ms"} />
            <.kv label="B3 batch" value={"#{pipeline_field(@dashboard_state, :b3_batch_size, 0)} chunks / #{pipeline_field(@dashboard_state, :b3_batch_timeout_ms, 0)} ms"} />
            <.kv label="Last lane" value={embedder_field(@dashboard_state, :last_lane, "unknown")} />
          </div>
        </section>

        <%!-- INDEXER WORKER ACTIVITY --%>
        <section class="col-span-12 lg:col-span-7 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="px-5 py-3 border-b border-slate-800">
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Worker Activity</div>
            <h2 class="text-base font-semibold text-slate-100 mt-0.5">Graph workers, queues, ingress</h2>
          </header>
          <div class="grid grid-cols-2 md:grid-cols-3 gap-3 p-4">
            <.kpi
              label="Graph Workers"
              value={"#{telemetry_field(@dashboard_state, :graph_workers_active_current, 0)} / #{telemetry_field(@dashboard_state, :graph_workers_started_total, 0)}"}
              sub="active / started"
              tone={:neutral}
            />
            <.kpi
              label="Ingress Buffered"
              value={telemetry_field(@dashboard_state, :ingress_buffered_entries, 0) |> full_int()}
              sub={"#{telemetry_field(@dashboard_state, :ingress_hot_entries, 0)} hot"}
              tone={:neutral}
            />
            <.kpi
              label="Ready Chunks"
              value={telemetry_field(@dashboard_state, :ready_queue_chunks_current, 0) |> full_int()}
              sub={"S #{telemetry_field(@dashboard_state, :ready_queue_chunks_small, 0)} M #{telemetry_field(@dashboard_state, :ready_queue_chunks_medium, 0)} L #{telemetry_field(@dashboard_state, :ready_queue_chunks_large, 0)}"}
              tone={:neutral}
            />
            <.kpi
              label="Batch Shape"
              value={"H #{telemetry_field(@dashboard_state, :homogeneous_batches_total, 0)} / M #{telemetry_field(@dashboard_state, :mixed_fallback_batches_total, 0)}"}
              sub="homogeneous / mixed"
              tone={:neutral}
            />
          </div>
        </section>

        <%!-- WORKER CONFIG TABLE --%>
        <section class="col-span-12 lg:col-span-5 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="px-5 py-3 border-b border-slate-800">
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Worker Configuration</div>
            <h2 class="text-base font-semibold text-slate-100 mt-0.5">From runtime_config (boot env-resolved)</h2>
          </header>
          <div class="overflow-hidden">
            <table class="w-full text-sm">
              <thead class="bg-slate-950/40 text-[10px] uppercase tracking-wider text-slate-500">
                <tr>
                  <th class="px-4 py-2 text-left">Stage</th>
                  <th class="px-4 py-2 text-left">Purpose</th>
                  <th class="px-4 py-2 text-right">Workers</th>
                  <th class="px-4 py-2 text-right">Batch</th>
                </tr>
              </thead>
              <tbody class="font-mono text-xs divide-y divide-slate-800/60">
                <.stage_row name="A1" tone={:cyan} purpose="read + hash" workers={pipeline_field(@dashboard_state, :a1_workers, 0)} batch="—" />
                <.stage_row name="A2" tone={:cyan} purpose="parse TS" workers={pipeline_field(@dashboard_state, :a2_workers, 0)} batch="—" />
                <.stage_row name="A3" tone={:cyan} purpose="graph UPSERT" workers={pipeline_field(@dashboard_state, :a3_workers, 0)} batch={"#{pipeline_field(@dashboard_state, :a3_batch_size, 0)} / #{pipeline_field(@dashboard_state, :a3_batch_timeout_ms, 0)} ms"} />
                <.stage_row name="B1" tone={:emerald} purpose="fetch chunks" workers={pipeline_field(@dashboard_state, :b1_workers, 0)} batch="—" />
                <.stage_row name="B2" tone={:emerald} purpose="embed GPU" workers={pipeline_field(@dashboard_state, :b2_workers, 0)} batch={"#{pipeline_field(@dashboard_state, :b2_batch_size, 0)} / #{pipeline_field(@dashboard_state, :b2_batch_timeout_ms, 0)} ms"} />
                <.stage_row name="B3" tone={:emerald} purpose="write embeddings" workers={pipeline_field(@dashboard_state, :b3_workers, 0)} batch={"#{pipeline_field(@dashboard_state, :b3_batch_size, 0)} / #{pipeline_field(@dashboard_state, :b3_batch_timeout_ms, 0)} ms"} />
              </tbody>
            </table>
            <div class="px-4 py-3 bg-amber-950/20 border-t border-amber-900/40 text-[10px] text-amber-300/80">
              <strong class="font-semibold">Note:</strong> per-stage items_in/out/inflight/backpressure counters
              are not exported on the runtime surface yet (only the bench reads <code class="bg-slate-900 px-1 rounded">StageSnapshot</code>).
            </div>
          </div>
        </section>

        <%!-- PER-PROJECT BREAKDOWN (Phoenix.LiveView.stream) --%>
        <section class="col-span-12 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="px-5 py-3 border-b border-slate-800">
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Per-Project Breakdown</div>
            <h2 class="text-base font-semibold text-slate-100 mt-0.5">Indexed chunks, embeddings, symbols by project</h2>
          </header>
          <div class="overflow-hidden">
            <table class="w-full text-sm">
              <thead class="bg-slate-950/40 text-[10px] uppercase tracking-wider text-slate-500">
                <tr>
                  <th class="px-4 py-2 text-left">Project</th>
                  <th class="px-4 py-2 text-right">Symbols</th>
                  <th class="px-4 py-2 text-right">Chunks</th>
                  <th class="px-4 py-2 text-right">Embeddings</th>
                  <th class="px-4 py-2 text-right">Coverage</th>
                </tr>
              </thead>
              <tbody id="per_project_rows" phx-update="stream" class="font-mono text-xs divide-y divide-slate-800/60">
                <tr :for={{dom_id, entry} <- @streams.per_project} id={dom_id} class="hover:bg-slate-800/30 transition-colors">
                  <td class="px-4 py-2">
                    <span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-[10px] uppercase font-semibold tracking-wide border border-cyan-500/30 bg-cyan-500/5 text-cyan-200">
                      {entry.project_code}
                    </span>
                  </td>
                  <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{full_int(entry.symbols)}</td>
                  <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{full_int(entry.chunks)}</td>
                  <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{full_int(entry.embedded)}</td>
                  <td class="px-4 py-2 text-right tabular-nums">
                    <span class={coverage_text_class(entry.coverage_pct)}>
                      {:erlang.float_to_binary(entry.coverage_pct * 1.0, decimals: 2)}%
                    </span>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>
      </div>
    </Nav.shell>
    """
  end

  ## Components

  attr :label, :string, required: true
  attr :value, :string, required: true
  attr :sub, :string, default: nil
  attr :tone, :atom, default: :neutral

  defp kpi(assigns) do
    ~H"""
    <div class={[
      "rounded-lg border bg-slate-900/50 backdrop-blur-sm px-4 py-3",
      kpi_border(@tone)
    ]}>
      <div class="text-[10px] uppercase tracking-[0.14em] text-slate-500">{@label}</div>
      <div class={[
        "mt-1 font-mono font-semibold tabular-nums text-2xl leading-none",
        kpi_text(@tone)
      ]}>{@value}</div>
      <div :if={@sub} class="mt-1 text-[10px] font-mono text-slate-500 truncate">{@sub}</div>
    </div>
    """
  end

  defp kpi_border(:ok), do: "border-emerald-500/30"
  defp kpi_border(:warn), do: "border-amber-500/40"
  defp kpi_border(:danger), do: "border-red-500/40"
  defp kpi_border(_), do: "border-slate-800/80"

  defp kpi_text(:ok), do: "text-emerald-300"
  defp kpi_text(:warn), do: "text-amber-300"
  defp kpi_text(:danger), do: "text-red-300"
  defp kpi_text(_), do: "text-slate-100"

  attr :label, :string, required: true
  attr :value, :string, required: true

  defp kv(assigns) do
    ~H"""
    <div class="flex items-center justify-between gap-3 text-[11px] font-mono py-1.5 border-b border-slate-800/40 last:border-0">
      <span class="uppercase tracking-wider text-slate-500">{@label}</span>
      <strong class="text-slate-100 text-right truncate">{@value}</strong>
    </div>
    """
  end

  attr :name, :string, required: true
  attr :purpose, :string, required: true
  attr :workers, :integer, required: true
  attr :batch, :string, required: true
  attr :tone, :atom, default: :cyan

  defp stage_row(assigns) do
    ~H"""
    <tr class="hover:bg-slate-800/30 transition-colors">
      <td class="px-4 py-2">
        <span class={[
          "inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-[10px] uppercase font-semibold tracking-wide border",
          stage_class(@tone)
        ]}>
          <span class={["h-1.5 w-1.5 rounded-full", stage_dot(@tone)]}></span>
          {@name}
        </span>
      </td>
      <td class="px-4 py-2 text-slate-300">{@purpose}</td>
      <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{@workers}</td>
      <td class="px-4 py-2 text-right text-slate-400 text-[11px]">{@batch}</td>
    </tr>
    """
  end

  defp stage_class(:cyan), do: "border-cyan-500/30 bg-cyan-500/5 text-cyan-200"
  defp stage_class(:emerald), do: "border-emerald-500/30 bg-emerald-500/5 text-emerald-200"

  defp stage_dot(:cyan), do: "bg-cyan-400"
  defp stage_dot(:emerald), do: "bg-emerald-400"

  ## Struct accessors — atom keys, nil-safe

  defp runtime_field(%DashboardState{runtime: nil}, _key, default), do: default
  defp runtime_field(%DashboardState{runtime: r}, key, default), do: Map.get(r, key, default) || default

  defp embedder_field(%DashboardState{embedder: nil}, _key, default), do: default
  defp embedder_field(%DashboardState{embedder: e}, key, default), do: Map.get(e, key, default) || default

  defp telemetry_field(%DashboardState{telemetry: nil}, _key, default), do: default
  defp telemetry_field(%DashboardState{telemetry: t}, key, default), do: Map.get(t, key, default)

  defp totals_field(%DashboardState{totals: nil}, _key, default), do: default
  defp totals_field(%DashboardState{totals: t}, key, default), do: Map.get(t, key, default)

  defp lifecycle_field(%DashboardState{lifecycle: nil}, _key, default), do: default
  defp lifecycle_field(%DashboardState{lifecycle: l}, key, default), do: Map.get(l, key, default) || default

  defp pipeline_field(%DashboardState{runtime_config: nil}, _key, default), do: default
  defp pipeline_field(%DashboardState{runtime_config: rc}, key, default) do
    Map.get(rc.pipeline, key, default)
  end

  defp rc_field(%DashboardState{runtime_config: nil}, _key, default), do: default
  defp rc_field(%DashboardState{runtime_config: rc}, key, default), do: Map.get(rc, key, default) || default

  ## Formatting

  # REQ-AXO-901827 — opérateur veut la valeur entière brute (pas
  # de format "14.9k" / "1.99M") tant que l'indexer sous-indexe et
  # qu'il faut compter manuellement les écarts vs PG truth.
  defp full_int(n) when is_integer(n), do: Integer.to_string(n)
  defp full_int(n) when is_float(n), do: Integer.to_string(round(n))
  defp full_int(_), do: "0"

  defp format_float(n) when is_float(n), do: :erlang.float_to_binary(n, decimals: 1)
  defp format_float(n) when is_integer(n), do: :erlang.float_to_binary(n * 1.0, decimals: 1)
  defp format_float(_), do: "0.0"

  defp coverage_tone(pct) when is_number(pct) and pct >= 95.0, do: :ok
  defp coverage_tone(pct) when is_number(pct) and pct >= 75.0, do: :neutral
  defp coverage_tone(_), do: :warn

  defp pending_tone(%DashboardState{totals: nil}), do: :ok
  defp pending_tone(%DashboardState{totals: %{pending: 0}}), do: :ok
  defp pending_tone(%DashboardState{totals: %{pending: n}}) when is_number(n) and n < 1000, do: :neutral
  defp pending_tone(_), do: :warn

  defp pressure_class("healthy"), do: "text-emerald-300"
  defp pressure_class("warm"), do: "text-amber-300"
  defp pressure_class("hot"), do: "text-red-300"
  defp pressure_class(_), do: "text-slate-300"

  defp embedder_class("cuda"), do: "border-emerald-500/40 bg-emerald-500/10 text-emerald-200"
  defp embedder_class("tensorrt"), do: "border-emerald-500/40 bg-emerald-500/10 text-emerald-200"
  defp embedder_class("cpu"), do: "border-amber-500/40 bg-amber-500/10 text-amber-200"
  defp embedder_class(_), do: "border-slate-700 bg-slate-800/40 text-slate-300"

  defp fs_val(%DashboardState{filesystem: nil}, _key), do: "n/a"
  defp fs_val(%DashboardState{filesystem: fs}, key) do
    case Map.get(fs, key) do
      n when is_integer(n) and n >= 0 -> full_int(n)
      _ -> "n/a"
    end
  end

  defp coverage_text_class(pct) when is_number(pct) and pct >= 95.0, do: "text-emerald-300"
  defp coverage_text_class(pct) when is_number(pct) and pct >= 75.0, do: "text-slate-100"
  defp coverage_text_class(_), do: "text-amber-300"

  ## Rate sparkline

  defp update_rate_series(socket, %DashboardState{telemetry: nil}), do: socket

  defp update_rate_series(socket, %DashboardState{telemetry: t}) do
    rate = t.chunk_embeddings_per_second || 0.0
    q = socket.assigns.rate_series

    q1 =
      :queue.in(rate, q)
      |> trim_queue(@rate_window_size)

    assign(socket, :rate_series, q1)
  end

  defp trim_queue(q, max) do
    if :queue.len(q) > max do
      {_, q1} = :queue.out(q)
      trim_queue(q1, max)
    else
      q
    end
  end

  defp rate_bar_heights(q) do
    list = :queue.to_list(q)
    list = list ++ List.duplicate(0.0, max(@rate_window_size - length(list), 0))

    max_val =
      list
      |> Enum.max(fn -> 1.0 end)
      |> Kernel.+(0.0001)

    Enum.map(list, fn v ->
      pct = v / max_val * 100
      pct |> max(2) |> min(100) |> round()
    end)
  end

  ## JS push throttle

  defp maybe_push_pipeline_state(socket) do
    now = System.monotonic_time(:millisecond)
    delta = now - socket.assigns.last_push_ms

    if delta >= @push_min_interval_ms do
      push_pipeline_state(socket, force: false)
    else
      socket
    end
  end

  defp push_pipeline_state(socket, opts) do
    if connected?(socket) or Keyword.get(opts, :force, false) do
      payload = pipeline_state_payload(socket.assigns.dashboard_state)
      now = System.monotonic_time(:millisecond)

      socket
      |> push_event("pipeline_state", payload)
      |> assign(:last_push_ms, now)
    else
      socket
    end
  end

  defp pipeline_state_payload(%DashboardState{} = s) do
    %{
      stages: [
        %{id: "a1", label: "A1 read+hash", workers: pipeline_field(s, :a1_workers, 0), group: "A"},
        %{id: "a2", label: "A2 parse-TS", workers: pipeline_field(s, :a2_workers, 0), group: "A"},
        %{id: "a3", label: "A3 graph-UPSERT", workers: pipeline_field(s, :a3_workers, 0), group: "A"},
        %{id: "b1", label: "B1 fetch", workers: pipeline_field(s, :b1_workers, 0), group: "B"},
        %{id: "b2", label: "B2 embed-GPU", workers: pipeline_field(s, :b2_workers, 0), group: "B"},
        %{id: "b3", label: "B3 write-emb", workers: pipeline_field(s, :b3_workers, 0), group: "B"}
      ],
      buffer: %{
        cap: pipeline_field(s, :a3_to_b1_buffer_cap, 0),
        fill: 0
      },
      rate: telemetry_field(s, :chunk_embeddings_per_second, 0.0),
      coverage_pct: totals_field(s, :coverage_pct, 0.0),
      graph_workers_active: telemetry_field(s, :graph_workers_active_current, 0),
      ingress_hot: telemetry_field(s, :ingress_hot_entries, 0),
      gpu: embedder_field(s, :effective, "unknown"),
      degraded: runtime_field(s, :degraded_reason, nil)
    }
  end
end
