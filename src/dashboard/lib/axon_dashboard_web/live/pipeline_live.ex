defmodule AxonDashboardWeb.Live.PipelineLive do
  @moduledoc """
  REQ-AXO-901647 page 1 — Pipeline V2 streaming cockpit.

  Renders the canonical CPT-AXO-054 topology

      A1 read+hash → A2 parse-TS → A3 graph-UPSERT
        ─try_send (A3→B1 cap 10000)──▶
      B1 fetch    → B2 embed-GPU → B3 write-emb

  Worker counts and batch sizes come from MCP `embedding_status`
  (via `Axon.Watcher.McpPoller`), while live deltas (chunks/sec,
  graph workers active, file vec queue, ingress hot) come from the
  indexer heartbeat JSON (via `Axon.Watcher.IndexerHeartbeat`).

  Per-stage in/out/inflight/backpressure counters do NOT currently
  exist in the runtime surface — only the bench (`axon-bench-pipeline-v2`)
  reads `StageSnapshot`. The dashboard surfaces the missing-counter
  state explicitly rather than fabricate values.
  """
  use Phoenix.LiveView

  alias Axon.Watcher.{IndexerHeartbeat, McpPoller}
  alias AxonDashboardWeb.Live.Nav

  @rate_window_size 60
  @push_min_interval_ms 250

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
    end

    hb = IndexerHeartbeat.latest()
    mcp = McpPoller.latest()

    socket =
      socket
      |> assign(:page_title, "Axon · Pipeline Cockpit")
      |> assign(:heartbeat, hb)
      |> assign(:mcp, mcp)
      # REQ-AXO-901806 — single-event dashboard_state_v1 assign,
      # initially nil until first :dashboard_state PubSub event lands.
      |> assign(:dashboard_state, nil)
      |> assign(:rate_series, :queue.new())
      |> assign(:last_push_ms, 0)
      |> assign(:total_chunks_history, :queue.new())

    {:ok, push_pipeline_state(socket, force: true)}
  end

  @impl true
  def handle_info({:indexer_heartbeat, snap}, socket) do
    socket =
      socket
      |> assign(:heartbeat, snap)
      |> update_rate_series(snap)
      |> maybe_push_pipeline_state()

    {:noreply, socket}
  end

  @impl true
  def handle_info({:indexer_heartbeat_stale, snap}, socket) do
    {:noreply, assign(socket, :heartbeat, snap)}
  end

  @impl true
  def handle_info({:indexer_heartbeat_missing, _info}, socket) do
    {:noreply, assign(socket, :heartbeat, %{status: :missing})}
  end

  @impl true
  def handle_info({:mcp_embedding_status, snap}, socket) do
    socket =
      socket
      |> assign(:mcp, snap)
      |> push_pipeline_state(force: true)

    {:noreply, socket}
  end

  @impl true
  def handle_info({:mcp_embedding_status_error, _reason}, socket) do
    {:noreply, socket}
  end

  # REQ-AXO-901806 — dashboard_state_v1 event handler (single-event
  # architecture). Stored as `@dashboard_state` for future migration ;
  # render still reads @heartbeat + @mcp during the dual-source window.
  # Once render is migrated, the IndexerHeartbeat + McpPoller pollers
  # in the supervision tree become removable (F6).
  @impl true
  def handle_info({:dashboard_state, state}, socket) do
    {:noreply, assign(socket, :dashboard_state, state)}
  end

  # Catch-all: keep process alive on any other broadcast
  def handle_info(_msg, socket), do: {:noreply, socket}

  @impl true
  def render(assigns) do
    ~H"""
    <Nav.shell
      current={:pipeline}
      build_id={hb_get(@heartbeat, :build_id, "n/a")}
      install_generation={hb_install_generation(@heartbeat)}
      runtime_mode={hb_get(@heartbeat, :runtime_mode, "unknown")}
      instance_kind={hb_instance_kind(@heartbeat)}
      gpu_effective={hb_gpu_effective(@heartbeat)}
      degraded_reason={hb_get(@heartbeat, :degraded_reason, nil)}
      stale={hb_stale?(@heartbeat)}
      observed_age_ms={hb_get(@heartbeat, :observed_age_ms, nil)}
    >
      <div class="grid grid-cols-12 gap-4">
        <%!-- INDEXATION FUNNEL --%>
        <section class="col-span-12 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm px-5 py-3">
          <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80 mb-2">Indexation Funnel</div>
          <div class="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-sm text-slate-200">
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Disk</span>
              <strong class="tabular-nums">{funnel_val(@mcp, :disk_files)}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Eligible</span>
              <strong class="tabular-nums">{funnel_val(@mcp, :eligible_files)}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Indexed</span>
              <strong class="tabular-nums">{mcp_get(@mcp, :indexed_files, 0) |> humanize_int()}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Chunks</span>
              <strong class="tabular-nums">{mcp_get(@mcp, :total_chunks, 0) |> humanize_int()}</strong>
            </span>
            <span class="text-slate-600">&rarr;</span>
            <span>
              <span class="text-slate-500 text-[10px] uppercase tracking-wider mr-1">Embeddings</span>
              <strong class={["tabular-nums", coverage_text_class(mcp_get(@mcp, :coverage_pct, 0.0))]}>
                {mcp_get(@mcp, :embedded_chunks, 0) |> humanize_int()}
              </strong>
              <span class="text-slate-500 text-[10px] ml-1">
                ({:erlang.float_to_binary(mcp_get(@mcp, :coverage_pct, 0.0) * 1.0, decimals: 1)}%)
              </span>
            </span>
          </div>
        </section>

        <%!-- COVERAGE SUMMARY --%>
        <section class="col-span-12 grid grid-cols-2 md:grid-cols-4 xl:grid-cols-6 gap-3">
          <.kpi label="Indexed Files" value={mcp_get(@mcp, :indexed_files, 0) |> humanize_int()} tone={:neutral} />
          <.kpi label="Symbols" value={mcp_get(@mcp, :symbols, 0) |> humanize_int()} tone={:neutral} />
          <.kpi label="Edges" value={mcp_get(@mcp, :edges, 0) |> humanize_int()} tone={:neutral} />
          <.kpi
            label="Total Chunks"
            value={mcp_get(@mcp, :total_chunks, 0) |> humanize_int()}
            tone={:neutral}
          />
          <.kpi
            label="Embedded"
            value={mcp_get(@mcp, :embedded_chunks, 0) |> humanize_int()}
            sub={"#{:erlang.float_to_binary(mcp_get(@mcp, :coverage_pct, 0.0) * 1.0, decimals: 2)}%"}
            tone={coverage_tone(mcp_get(@mcp, :coverage_pct, 0.0))}
          />
          <.kpi
            label="Pending"
            value={mcp_get(@mcp, :pending_chunks, 0) |> humanize_int()}
            sub={pending_sub(@mcp)}
            tone={pending_tone(@mcp)}
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
                lifecycle: <span class="text-slate-200">{mcp_get(@mcp, :lifecycle_phase, "?")}</span>
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
            <span>A3→B1 buffer cap: <strong class="text-slate-200">{mcp_get_in(@mcp, [:pipeline_b, :a3_to_b1_buffer_cap], 0) |> humanize_int()}</strong></span>
            <span>NOTIFY: <strong class="text-slate-200">{mcp_get(@mcp, :notify_channel, "n/a")}</strong></span>
            <span>coldstart poll: <strong class="text-slate-200">{mcp_get(@mcp, :coldstart_poll_interval_secs, 0)}s × {mcp_get_in(@mcp, [:pipeline_b, :coldstart_batch_size], 0) |> humanize_int()}</strong></span>
            <span>idle: <strong class={if mcp_get(@mcp, :runtime_idle, false), do: "text-emerald-300", else: "text-amber-300"}>{mcp_get(@mcp, :runtime_idle, false) |> to_string()}</strong></span>
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
                {format_float(hb_get_in(@heartbeat, [:telemetry, :chunk_embeddings_per_second], 0.0))}
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
              <span>chunks embedded (Σ): <strong class="text-slate-200">{hb_get_in(@heartbeat, [:telemetry, :vector_chunks_embedded_total], 0) |> humanize_int()}</strong></span>
              <span>scheduler: <strong class="text-slate-200">{hb_get_in(@heartbeat, [:telemetry, :utility_first_scheduler_state], "?")}</strong></span>
              <span>pressure: <strong class={pressure_class(hb_get_in(@heartbeat, [:telemetry, :service_pressure], "?"))}>{hb_get_in(@heartbeat, [:telemetry, :service_pressure], "?")}</strong></span>
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
              embedder_class(hb_gpu_effective(@heartbeat))
            ]}>
              {hb_gpu_effective(@heartbeat)}
            </span>
          </header>
          <div class="p-4 space-y-3">
            <.kv label="Requested provider" value={hb_get_in(@heartbeat, [:embedder_provider, :requested], "n/a")} />
            <.kv label="Effective provider" value={hb_get_in(@heartbeat, [:embedder_provider, :effective], "n/a")} />
            <.kv label="Init error" value={hb_get_in(@heartbeat, [:embedder_provider, :init_error], "—") || "—"} />
            <.kv label="B2 batch" value={"#{mcp_get_in(@mcp, [:pipeline_b, :b2_batch_size], 0)} chunks / #{mcp_get_in(@mcp, [:pipeline_b, :b2_batch_timeout_ms], 0)} ms"} />
            <.kv label="B3 batch" value={"#{mcp_get_in(@mcp, [:pipeline_b, :b3_batch_size], 0)} chunks / #{mcp_get_in(@mcp, [:pipeline_b, :b3_batch_timeout_ms], 0)} ms"} />
            <.kv label="Last lane" value={hb_get_in(@heartbeat, [:telemetry, :last_consumed_batch_lane], "unknown")} />
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
              value={"#{hb_get_in(@heartbeat, [:telemetry, :graph_workers_active_current], 0)} / #{hb_get_in(@heartbeat, [:telemetry, :graph_workers_started_total], 0)}"}
              sub="active / started"
              tone={:neutral}
            />
            <.kpi
              label="Ingress Buffered"
              value={hb_get_in(@heartbeat, [:telemetry, :ingress_buffered_entries], 0) |> humanize_int()}
              sub={"#{hb_get_in(@heartbeat, [:telemetry, :ingress_hot_entries], 0)} hot"}
              tone={:neutral}
            />
            <.kpi
              label="Ready Chunks"
              value={hb_get_in(@heartbeat, [:telemetry, :ready_queue_chunks_current], 0) |> humanize_int()}
              sub={"S #{hb_get_in(@heartbeat, [:telemetry, :ready_queue_chunks_small], 0)} M #{hb_get_in(@heartbeat, [:telemetry, :ready_queue_chunks_medium], 0)} L #{hb_get_in(@heartbeat, [:telemetry, :ready_queue_chunks_large], 0)}"}
              tone={:neutral}
            />
            <.kpi
              label="Batch Shape"
              value={"H #{hb_get_in(@heartbeat, [:telemetry, :homogeneous_batches_total], 0)} / M #{hb_get_in(@heartbeat, [:telemetry, :mixed_fallback_batches_total], 0)}"}
              sub="homogeneous / mixed"
              tone={:neutral}
            />
          </div>
        </section>

        <%!-- WORKER CONFIG TABLE --%>
        <section class="col-span-12 lg:col-span-5 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="px-5 py-3 border-b border-slate-800">
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Worker Configuration</div>
            <h2 class="text-base font-semibold text-slate-100 mt-0.5">From embedding_status (env-resolved)</h2>
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
                <.stage_row name="A1" tone={:cyan} purpose="read + hash" workers={mcp_get_in(@mcp, [:pipeline_a, :a1_workers], 0)} batch="—" />
                <.stage_row name="A2" tone={:cyan} purpose="parse TS" workers={mcp_get_in(@mcp, [:pipeline_a, :a2_workers], 0)} batch="—" />
                <.stage_row name="A3" tone={:cyan} purpose="graph UPSERT" workers={mcp_get_in(@mcp, [:pipeline_a, :a3_workers], 0)} batch={"#{mcp_get_in(@mcp, [:pipeline_a, :a3_batch_size], 0)} / #{mcp_get_in(@mcp, [:pipeline_a, :a3_batch_timeout_ms], 0)} ms"} />
                <.stage_row name="B1" tone={:emerald} purpose="fetch chunks" workers={mcp_get_in(@mcp, [:pipeline_b, :b1_workers], 0)} batch="—" />
                <.stage_row name="B2" tone={:emerald} purpose="embed GPU" workers={mcp_get_in(@mcp, [:pipeline_b, :b2_workers], 0)} batch={"#{mcp_get_in(@mcp, [:pipeline_b, :b2_batch_size], 0)} / #{mcp_get_in(@mcp, [:pipeline_b, :b2_batch_timeout_ms], 0)} ms"} />
                <.stage_row name="B3" tone={:emerald} purpose="write embeddings" workers={mcp_get_in(@mcp, [:pipeline_b, :b3_workers], 0)} batch={"#{mcp_get_in(@mcp, [:pipeline_b, :b3_batch_size], 0)} / #{mcp_get_in(@mcp, [:pipeline_b, :b3_batch_timeout_ms], 0)} ms"} />
              </tbody>
            </table>
            <div class="px-4 py-3 bg-amber-950/20 border-t border-amber-900/40 text-[10px] text-amber-300/80">
              <strong class="font-semibold">Note:</strong> per-stage items_in/out/inflight/backpressure counters
              are not exported on the runtime surface yet (only the bench reads <code class="bg-slate-900 px-1 rounded">StageSnapshot</code>).
              Add a counter export to expose live per-stage deltas.
            </div>
          </div>
        </section>

        <%!-- PER-PROJECT BREAKDOWN --%>
        <section :if={has_per_project?(@mcp)} class="col-span-12 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm">
          <header class="px-5 py-3 border-b border-slate-800">
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Per-Project Breakdown</div>
            <h2 class="text-base font-semibold text-slate-100 mt-0.5">Indexed files, chunks, embeddings by project</h2>
          </header>
          <div class="overflow-hidden">
            <table class="w-full text-sm">
              <thead class="bg-slate-950/40 text-[10px] uppercase tracking-wider text-slate-500">
                <tr>
                  <th class="px-4 py-2 text-left">Project</th>
                  <th class="px-4 py-2 text-right">Indexed Files</th>
                  <th class="px-4 py-2 text-right">Chunks</th>
                  <th class="px-4 py-2 text-right">Embeddings</th>
                  <th class="px-4 py-2 text-right">Coverage</th>
                </tr>
              </thead>
              <tbody class="font-mono text-xs divide-y divide-slate-800/60">
                <%= for entry <- mcp_get(@mcp, :per_project, []) do %>
                  <tr class="hover:bg-slate-800/30 transition-colors">
                    <td class="px-4 py-2">
                      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-[10px] uppercase font-semibold tracking-wide border border-cyan-500/30 bg-cyan-500/5 text-cyan-200">
                        {Map.get(entry, :project_code, "?")}
                      </span>
                    </td>
                    <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{Map.get(entry, :indexed_files, 0) |> humanize_int()}</td>
                    <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{Map.get(entry, :chunks, 0) |> humanize_int()}</td>
                    <td class="px-4 py-2 text-right text-slate-100 tabular-nums">{Map.get(entry, :embeddings, 0) |> humanize_int()}</td>
                    <td class="px-4 py-2 text-right tabular-nums">
                      <span class={coverage_text_class(Map.get(entry, :coverage_pct, 0.0))}>
                        {:erlang.float_to_binary(Map.get(entry, :coverage_pct, 0.0) * 1.0, decimals: 2)}%
                      </span>
                    </td>
                  </tr>
                <% end %>
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

  ## Helpers - assigns accessors

  defp hb_get(nil, _key, default), do: default
  defp hb_get(hb, key, default), do: Map.get(hb, key, default) || default

  defp hb_get_in(nil, _path, default), do: default

  defp hb_get_in(hb, path, default) do
    case get_in(hb, path) do
      nil -> default
      val -> val
    end
  end

  defp hb_stale?(nil), do: true
  defp hb_stale?(%{status: :missing}), do: true
  defp hb_stale?(%{stale: true}), do: true
  defp hb_stale?(_), do: false

  defp hb_install_generation(hb), do: hb_get(hb, :install_generation, hb_get(hb, :release_version, "n/a"))

  defp hb_instance_kind(hb) do
    case Application.get_env(:axon_dashboard, :instance_kind) do
      nil -> hb_get(hb, :runtime_mode, "unknown")
      kind -> kind
    end
  end

  defp hb_gpu_effective(hb), do: hb_get_in(hb, [:embedder_provider, :effective], "unknown") || "unknown"

  defp mcp_get(nil, _key, default), do: default
  defp mcp_get(mcp, key, default), do: Map.get(mcp, key, default) || default

  defp mcp_get_in(nil, _path, default), do: default

  defp mcp_get_in(mcp, path, default) do
    case get_in(mcp, path) do
      nil -> default
      val -> val
    end
  end

  ## Helpers - formatting

  defp humanize_int(n) when is_integer(n) and n >= 1_000_000, do: "#{Float.round(n / 1_000_000, 2)}M"
  defp humanize_int(n) when is_integer(n) and n >= 10_000, do: "#{Float.round(n / 1_000, 1)}k"
  defp humanize_int(n) when is_integer(n), do: Integer.to_string(n)
  defp humanize_int(n) when is_float(n), do: humanize_int(round(n))
  defp humanize_int(_), do: "0"

  defp format_float(n) when is_float(n), do: :erlang.float_to_binary(n, decimals: 1)
  defp format_float(n) when is_integer(n), do: :erlang.float_to_binary(n * 1.0, decimals: 1)
  defp format_float(_), do: "0.0"

  defp coverage_tone(pct) when is_number(pct) and pct >= 95.0, do: :ok
  defp coverage_tone(pct) when is_number(pct) and pct >= 75.0, do: :neutral
  defp coverage_tone(_), do: :warn

  defp pending_sub(mcp) do
    rate = mcp_get(mcp, :runtime_pending_count, 0)
    "runtime set: #{humanize_int(rate)}"
  end

  defp pending_tone(mcp) do
    case mcp_get(mcp, :pending_chunks, 0) do
      0 -> :ok
      n when is_number(n) and n < 1000 -> :neutral
      _ -> :warn
    end
  end

  defp pressure_class("healthy"), do: "text-emerald-300"
  defp pressure_class("warm"), do: "text-amber-300"
  defp pressure_class("hot"), do: "text-red-300"
  defp pressure_class(_), do: "text-slate-300"

  defp embedder_class("cuda"), do: "border-emerald-500/40 bg-emerald-500/10 text-emerald-200"
  defp embedder_class("tensorrt"), do: "border-emerald-500/40 bg-emerald-500/10 text-emerald-200"
  defp embedder_class("cpu"), do: "border-amber-500/40 bg-amber-500/10 text-amber-200"
  defp embedder_class(_), do: "border-slate-700 bg-slate-800/40 text-slate-300"

  defp funnel_val(mcp, key) do
    case mcp_get(mcp, key, 0) do
      n when is_number(n) and n >= 0 -> humanize_int(n)
      _ -> "n/a"
    end
  end

  defp has_per_project?(mcp) do
    case mcp_get(mcp, :per_project, []) do
      list when is_list(list) and length(list) > 0 -> true
      _ -> false
    end
  end

  defp coverage_text_class(pct) when is_number(pct) and pct >= 95.0, do: "text-emerald-300"
  defp coverage_text_class(pct) when is_number(pct) and pct >= 75.0, do: "text-slate-100"
  defp coverage_text_class(_), do: "text-amber-300"

  ## Rate sparkline

  defp update_rate_series(socket, snap) do
    rate = get_in(snap, [:telemetry, :chunk_embeddings_per_second]) || 0.0
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
      payload = pipeline_state_payload(socket.assigns.mcp, socket.assigns.heartbeat)
      now = System.monotonic_time(:millisecond)

      socket
      |> push_event("pipeline_state", payload)
      |> assign(:last_push_ms, now)
    else
      socket
    end
  end

  defp pipeline_state_payload(mcp, hb) do
    %{
      stages: [
        %{id: "a1", label: "A1 read+hash", workers: mcp_get_in(mcp, [:pipeline_a, :a1_workers], 0), group: "A"},
        %{id: "a2", label: "A2 parse-TS", workers: mcp_get_in(mcp, [:pipeline_a, :a2_workers], 0), group: "A"},
        %{id: "a3", label: "A3 graph-UPSERT", workers: mcp_get_in(mcp, [:pipeline_a, :a3_workers], 0), group: "A"},
        %{id: "b1", label: "B1 fetch", workers: mcp_get_in(mcp, [:pipeline_b, :b1_workers], 0), group: "B"},
        %{id: "b2", label: "B2 embed-GPU", workers: mcp_get_in(mcp, [:pipeline_b, :b2_workers], 0), group: "B"},
        %{id: "b3", label: "B3 write-emb", workers: mcp_get_in(mcp, [:pipeline_b, :b3_workers], 0), group: "B"}
      ],
      buffer: %{
        cap: mcp_get_in(mcp, [:pipeline_b, :a3_to_b1_buffer_cap], 0),
        fill: 0
      },
      rate: hb_get_in(hb, [:telemetry, :chunk_embeddings_per_second], 0.0) || 0.0,
      coverage_pct: mcp_get(mcp, :coverage_pct, 0.0) || 0.0,
      graph_workers_active: hb_get_in(hb, [:telemetry, :graph_workers_active_current], 0) || 0,
      ingress_hot: hb_get_in(hb, [:telemetry, :ingress_hot_entries], 0) || 0,
      gpu: hb_gpu_effective(hb),
      degraded: hb_get(hb, :degraded_reason, nil)
    }
  end
end
