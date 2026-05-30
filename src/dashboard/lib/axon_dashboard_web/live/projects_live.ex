defmodule AxonDashboardWeb.Live.ProjectsLive do
  @moduledoc """
  REQ-AXO-901647 page 2 — per-project indexing & embedding coverage.

  Source of truth: BridgeClient push (PubSub `dashboard_topic`), derived
  from `DashboardState.per_project_counts` populated by the brain 1 Hz
  composer. SqlGateway is only used as a one-shot fallback at mount
  when the BridgeClient has no fresh snapshot yet (cold-start gap).

  REQ-AXO-901834 — pure push pattern : eliminates the `:timer.send_interval`
  polling that was driving cluster head R1 per session 64 dashboard audit.
  Phoenix LiveView principle = state pushed via WebSocket diff ; polling
  is a fallback, not a design.

  Columns: project_code · chunks · embedded · coverage% · symbols · edges
  · last_chunk_at · last_embedded_at · Δ chunks (rolling 60s).
  """
  use Phoenix.LiveView

  alias AxonDashboard.{BridgeClient, DashboardState}
  alias Axon.Watcher.SqlGateway
  alias AxonDashboardWeb.Live.Nav

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, BridgeClient.dashboard_topic())
    end

    initial_state = BridgeClient.dashboard_state() || %DashboardState{}
    initial_projects =
      case projects_from_dashboard_state(initial_state) do
        [] ->
          # Cold-start gap : brain hasn't pushed yet. One-shot SqlGateway
          # fetch so the first paint isn't empty. Subsequent updates land
          # via PubSub.
          {projects, _err} = fetch_projects()
          projects

        rows ->
          rows
      end

    socket =
      socket
      |> assign(:page_title, "Axon · Projects")
      |> assign(:projects, initial_projects)
      |> assign(:fetch_error, nil)
      |> assign(:sort_by, :chunks)
      |> assign(:sort_dir, :desc)
      |> assign(:dashboard_state, initial_state)
      |> assign(:prev_snapshot, %{})
      |> assign(:last_refresh_ms, System.monotonic_time(:millisecond))

    {:ok, socket}
  end

  @impl true
  # REQ-AXO-901826 + REQ-AXO-901834 — typed struct pattern match. Push
  # carries per_project_counts ; ProjectsLive re-derives the table rows
  # without polling. coverage_pct computed client-side from chunks/embedded.
  def handle_info({:dashboard_state, %DashboardState{} = state}, socket) do
    new_projects = projects_from_dashboard_state(state)

    socket =
      if new_projects == [] do
        # Push payload lacks per_project (transient brain composer hiccup):
        # keep prior assigns rather than blank the table.
        assign(socket, :dashboard_state, state)
      else
        prev_snapshot = build_prev_map(socket.assigns.projects)

        socket
        |> assign(:dashboard_state, state)
        |> assign(:projects, new_projects)
        |> assign(:prev_snapshot, prev_snapshot)
        |> assign(:last_refresh_ms, System.monotonic_time(:millisecond))
        |> assign(:fetch_error, nil)
      end

    {:noreply, socket}
  end

  def handle_info(_, socket), do: {:noreply, socket}

  @impl true
  def handle_event("sort", %{"col" => col}, socket) do
    col_atom = String.to_atom(col)
    {sort_by, sort_dir} =
      if socket.assigns.sort_by == col_atom do
        {col_atom, flip(socket.assigns.sort_dir)}
      else
        {col_atom, :desc}
      end

    {:noreply,
     socket
     |> assign(:sort_by, sort_by)
     |> assign(:sort_dir, sort_dir)}
  end

  @impl true
  def render(assigns) do
    sorted = sort_projects(assigns.projects, assigns.sort_by, assigns.sort_dir)
    totals = compute_totals(assigns.projects)
    assigns = assign(assigns, :sorted, sorted) |> assign(:totals, totals)

    ~H"""
    <Nav.shell
      current={:projects}
      build_id={runtime_field(@dashboard_state, :build_id, "n/a")}
      install_generation={runtime_field(@dashboard_state, :install_generation, "n/a")}
      runtime_mode={runtime_field(@dashboard_state, :runtime_mode, "unknown")}
      instance_kind={runtime_field(@dashboard_state, :instance_kind, Application.get_env(:axon_dashboard, :instance_kind, "unknown"))}
      gpu_effective={embedder_field(@dashboard_state, :effective, "unknown")}
      degraded_reason={runtime_field(@dashboard_state, :degraded_reason, nil)}
      stale={is_nil(@dashboard_state.ts_ms)}
      observed_age_ms={DashboardState.observed_age_ms(@dashboard_state)}
    >
      <div class="grid grid-cols-12 gap-4">
        <%!-- TOTALS --%>
        <section class="col-span-12 grid grid-cols-2 md:grid-cols-5 gap-3">
          <.tot label="Projects" value={length(@projects)} />
          <.tot label="Σ Chunks" value={@totals.chunks |> hint()} />
          <.tot label="Σ Embedded" value={@totals.embedded |> hint()} sub={"#{coverage(@totals.embedded, @totals.chunks)}%"} />
          <.tot label="Σ Symbols" value={@totals.symbols |> hint()} />
          <.tot label="Σ Edges" value={@totals.edges |> hint()} />
        </section>

        <%!-- TABLE --%>
        <section class="col-span-12 rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm overflow-hidden">
          <header class="flex items-center justify-between px-5 py-3 border-b border-slate-800">
            <div>
              <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Indexing per project</div>
              <h2 class="text-base font-semibold text-slate-100 mt-0.5">{length(@projects)} project codes · refresh 2s</h2>
            </div>
            <div :if={@fetch_error} class="text-[11px] font-mono text-red-300">
              fetch error: {@fetch_error}
            </div>
          </header>

          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead class="bg-slate-950/40 text-[10px] uppercase tracking-wider text-slate-500 sticky top-0">
                <tr>
                  <.th col="project_code" label="Project" sort_by={@sort_by} sort_dir={@sort_dir} align="left" />
                  <.th col="chunks" label="Chunks" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                  <.th col="embedded" label="Embedded" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                  <.th col="coverage" label="Coverage" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                  <.th col="symbols" label="Symbols" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                  <.th col="edges" label="Edges" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                  <.th col="delta_chunks" label="Δ Chunks" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                  <.th col="delta_embedded" label="Δ Embedded" sort_by={@sort_by} sort_dir={@sort_dir} align="right" />
                </tr>
              </thead>
              <tbody class="font-mono text-xs divide-y divide-slate-800/60">
                <tr :for={p <- @sorted} class="hover:bg-slate-800/30 transition-colors">
                  <td class="px-4 py-2.5">
                    <span class="px-2 py-0.5 rounded text-[10px] uppercase font-semibold tracking-wide border border-slate-700 bg-slate-800/60 text-amber-300">
                      {p.project_code}
                    </span>
                  </td>
                  <td class="px-4 py-2.5 text-right text-slate-100 tabular-nums">{format_n(p.chunks)}</td>
                  <td class="px-4 py-2.5 text-right text-slate-300 tabular-nums">{format_n(p.embedded)}</td>
                  <td class="px-4 py-2.5 text-right">
                    <div class="inline-flex items-center gap-2">
                      <div class="w-20 h-1.5 bg-slate-800 rounded-full overflow-hidden">
                        <div
                          class={["h-full transition-all duration-500", coverage_bar(p.coverage_pct)]}
                          style={"width: #{p.coverage_pct}%"}
                        ></div>
                      </div>
                      <span class={[
                        "tabular-nums w-12 text-right",
                        coverage_text(p.coverage_pct)
                      ]}>
                        {Float.round(p.coverage_pct * 1.0, 1)}%
                      </span>
                    </div>
                  </td>
                  <td class="px-4 py-2.5 text-right text-slate-300 tabular-nums">{format_n(p.symbols)}</td>
                  <td class="px-4 py-2.5 text-right text-slate-300 tabular-nums">{format_n(p.edges)}</td>
                  <td class={["px-4 py-2.5 text-right tabular-nums", delta_class(delta(p.chunks, @prev_snapshot[p.project_code], :chunks))]}>
                    {delta_label(delta(p.chunks, @prev_snapshot[p.project_code], :chunks))}
                  </td>
                  <td class={["px-4 py-2.5 text-right tabular-nums", delta_class(delta(p.embedded, @prev_snapshot[p.project_code], :embedded))]}>
                    {delta_label(delta(p.embedded, @prev_snapshot[p.project_code], :embedded))}
                  </td>
                </tr>
                <tr :if={@projects == []}>
                  <td colspan="8" class="px-6 py-10 text-center text-slate-500 text-[12px]">
                    No projects found. Check that the SQL gateway is reachable at
                    <code class="text-slate-300">{Axon.Watcher.SqlGateway.source_info().configured_url}</code>.
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

  attr :col, :string, required: true
  attr :label, :string, required: true
  attr :sort_by, :atom, required: true
  attr :sort_dir, :atom, required: true
  attr :align, :string, default: "left"

  defp th(assigns) do
    ~H"""
    <th class={["px-4 py-2", "text-#{@align}", "select-none"]}>
      <button
        phx-click="sort"
        phx-value-col={@col}
        class="inline-flex items-center gap-1 hover:text-amber-300 transition-colors cursor-pointer"
      >
        {@label}
        <span :if={Atom.to_string(@sort_by) == @col} class="text-amber-400">
          {if @sort_dir == :desc, do: "▼", else: "▲"}
        </span>
      </button>
    </th>
    """
  end

  attr :label, :string, required: true
  attr :value, :any, required: true
  attr :sub, :string, default: nil

  defp tot(assigns) do
    ~H"""
    <div class="rounded-lg border border-slate-800/80 bg-slate-900/50 px-4 py-3">
      <div class="text-[10px] uppercase tracking-[0.14em] text-slate-500">{@label}</div>
      <div class="mt-1 font-mono font-semibold tabular-nums text-2xl leading-none text-slate-100">{@value}</div>
      <div :if={@sub} class="mt-1 text-[10px] font-mono text-emerald-300">{@sub}</div>
    </div>
    """
  end

  ## Data

  # REQ-AXO-901834 — derive table rows from the pushed DashboardState
  # without polling. Returns [] when the push payload has no per_project
  # entries (transient brain composer hiccup) ; caller decides whether
  # to blank the table or keep the prior assigns.
  defp projects_from_dashboard_state(%DashboardState{per_project: list})
       when is_list(list) and list != [] do
    Enum.map(list, fn entry ->
      %{
        project_code: Map.get(entry, :project_code, "?"),
        chunks: Map.get(entry, :chunks, 0),
        embedded: Map.get(entry, :embedded, 0),
        symbols: Map.get(entry, :symbols, 0),
        edges: Map.get(entry, :edges, 0),
        coverage_pct: Map.get(entry, :coverage_pct, 0.0)
      }
    end)
  end

  defp projects_from_dashboard_state(_), do: []

  defp fetch_projects do
    query = """
    SELECT
      c.project_code,
      COUNT(c.id)::bigint AS chunks,
      COUNT(ce.chunk_id)::bigint AS embedded,
      COALESCE(s.symbols, 0)::bigint AS symbols,
      COALESCE(e.edges, 0)::bigint AS edges
    FROM public.chunk c
    LEFT JOIN public.chunkembedding ce ON ce.chunk_id = c.id
    LEFT JOIN (SELECT project_code, COUNT(*) AS symbols FROM public.symbol GROUP BY 1) s ON s.project_code = c.project_code
    LEFT JOIN (SELECT project_code, COUNT(*) AS edges FROM public.edge GROUP BY 1) e ON e.project_code = c.project_code
    GROUP BY c.project_code, s.symbols, e.edges
    ORDER BY chunks DESC
    """

    case SqlGateway.query_json(query) do
      {:ok, body} ->
        case Jason.decode(body) do
          {:ok, rows} when is_list(rows) ->
            projects =
              Enum.map(rows, fn [code, chunks, embedded, symbols, edges] ->
                chunks_i = to_int(chunks)
                embedded_i = to_int(embedded)
                cov =
                  if chunks_i > 0, do: embedded_i * 100.0 / chunks_i, else: 0.0

                %{
                  project_code: to_string(code),
                  chunks: chunks_i,
                  embedded: embedded_i,
                  symbols: to_int(symbols),
                  edges: to_int(edges),
                  coverage_pct: cov
                }
              end)

            {projects, nil}

          {:ok, %{"error" => err}} ->
            {[], to_string(err)}

          {:error, reason} ->
            {[], inspect(reason)}
        end

      {:error, reason} ->
        {[], inspect(reason)}
    end
  end

  defp to_int(n) when is_integer(n), do: n
  defp to_int(n) when is_binary(n), do: String.to_integer(n)
  defp to_int(_), do: 0

  defp build_prev_map(projects) do
    Enum.into(projects, %{}, fn p ->
      {p.project_code, %{chunks: p.chunks, embedded: p.embedded}}
    end)
  end

  defp compute_totals(projects) do
    Enum.reduce(projects, %{chunks: 0, embedded: 0, symbols: 0, edges: 0}, fn p, acc ->
      %{
        chunks: acc.chunks + p.chunks,
        embedded: acc.embedded + p.embedded,
        symbols: acc.symbols + p.symbols,
        edges: acc.edges + p.edges
      }
    end)
  end

  ## Sorting

  defp sort_projects(projects, :coverage, dir),
    do: sort_by_key(projects, :coverage_pct, dir)

  defp sort_projects(projects, :delta_chunks, dir),
    do: sort_by_key(projects, :chunks, dir)

  defp sort_projects(projects, :delta_embedded, dir),
    do: sort_by_key(projects, :embedded, dir)

  defp sort_projects(projects, key, dir),
    do: sort_by_key(projects, key, dir)

  defp sort_by_key(list, :project_code, :asc),
    do: Enum.sort_by(list, & &1.project_code, :asc)

  defp sort_by_key(list, :project_code, :desc),
    do: Enum.sort_by(list, & &1.project_code, :desc)

  defp sort_by_key(list, key, :asc), do: Enum.sort_by(list, &Map.get(&1, key, 0), :asc)
  defp sort_by_key(list, key, :desc), do: Enum.sort_by(list, &Map.get(&1, key, 0), :desc)

  defp flip(:asc), do: :desc
  defp flip(_), do: :asc

  ## Deltas

  defp delta(_current, nil, _key), do: nil

  defp delta(current, prev, key) do
    current - Map.get(prev, key, current)
  end

  defp delta_label(nil), do: "—"
  defp delta_label(0), do: "0"
  defp delta_label(n) when n > 0, do: "+#{format_n(n)}"
  defp delta_label(n), do: "#{format_n(n)}"

  defp delta_class(nil), do: "text-slate-600"
  defp delta_class(0), do: "text-slate-500"
  defp delta_class(n) when n > 0, do: "text-emerald-400"
  defp delta_class(_), do: "text-amber-400"

  ## Formatting

  # REQ-AXO-901827 — valeur entière brute, pas de k/M abrégé.
  defp format_n(n) when is_integer(n), do: Integer.to_string(n)
  defp format_n(_), do: "0"

  defp hint(n) when is_integer(n), do: Integer.to_string(n)
  defp hint(n), do: to_string(n)

  defp coverage(_e, 0), do: 0.0
  defp coverage(e, c), do: Float.round(e * 100.0 / c, 2)

  defp coverage_bar(pct) when pct >= 95, do: "bg-gradient-to-r from-emerald-600 to-emerald-400"
  defp coverage_bar(pct) when pct >= 75, do: "bg-gradient-to-r from-amber-500 to-amber-300"
  defp coverage_bar(_), do: "bg-gradient-to-r from-red-600 to-red-400"

  defp coverage_text(pct) when pct >= 95, do: "text-emerald-300"
  defp coverage_text(pct) when pct >= 75, do: "text-amber-300"
  defp coverage_text(_), do: "text-red-300"

  ## DashboardState accessors (REQ-AXO-901826) — typed struct, atom keys.

  defp runtime_field(%DashboardState{runtime: nil}, _key, default), do: default
  defp runtime_field(%DashboardState{runtime: r}, key, default), do: Map.get(r, key, default) || default

  defp embedder_field(%DashboardState{embedder: nil}, _key, default), do: default
  defp embedder_field(%DashboardState{embedder: e}, key, default), do: Map.get(e, key, default) || default
end
