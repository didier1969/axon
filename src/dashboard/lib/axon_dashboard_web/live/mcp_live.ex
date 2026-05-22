defmodule AxonDashboardWeb.Live.McpLive do
  @moduledoc """
  REQ-AXO-901647 page 3 — MCP catalog.

  Lists the 68 public tools exposed by axon-brain MCP, with category
  grouping inferred from a static prefix dictionary + a one-line
  summary fetched from `help(tool=X)` on demand (cached).

  Refresh: every 30s for the tool list; help is lazy + cached.

  Search & filter client-side via Phoenix.LiveView assigns.
  """
  use Phoenix.LiveView

  alias Axon.Watcher.{IndexerHeartbeat, McpClient}
  alias AxonDashboardWeb.Live.Nav

  @refresh_ms 30_000

  # Category mapping derived from the public_tools list (status mode=verbose)
  # — categories aren't a runtime concept yet, so we project them client-side.
  @categories %{
    "help" => :system,
    "status" => :system,
    "mcp_surface_diagnostics" => :system,
    "skill_list" => :system,
    "skill_invoke" => :system,
    "prompt_template_get" => :system,
    "job_status" => :system,
    "fs_read" => :system,
    "project_status" => :system,
    "project_registry_lookup" => :system,
    "schema_overview" => :system,
    "list_labels_tables" => :system,
    "query_examples" => :system,
    "sql" => :system,
    "debug" => :system,
    "truth_check" => :system,
    "axon_init_project" => :system,
    "axon_apply_guidelines" => :system,
    "axon_apply_methodology_bundle" => :system,
    "axon_commit_work" => :system,
    "axon_pre_flight_check" => :system,
    "soll_manager" => :soll,
    "soll_apply_plan" => :soll,
    "soll_commit_revision" => :soll,
    "soll_query_context" => :soll,
    "soll_work_plan" => :soll,
    "soll_attach_evidence" => :soll,
    "soll_remove_evidence" => :soll,
    "soll_verify_requirements" => :soll,
    "soll_rollback_revision" => :soll,
    "soll_export" => :soll,
    "soll_generate_docs" => :soll,
    "soll_validate" => :soll,
    "soll_acyclic_audit" => :soll,
    "soll_relation_schema" => :soll,
    "snapshot_history" => :soll,
    "snapshot_diff" => :soll,
    "conception_view" => :soll,
    "restore_soll" => :soll,
    "infer_soll_mutation" => :soll,
    "refine_lattice" => :soll,
    "entrench_nuance" => :soll,
    "document_intent" => :soll,
    "re_anchor" => :soll,
    "change_safety" => :dx,
    "why" => :dx,
    "path" => :dx,
    "anomalies" => :dx,
    "retrieve_context" => :dx,
    "retrieve_context_layered" => :dx,
    "query" => :dx,
    "inspect" => :dx,
    "diagnose_indexing" => :dx,
    "embedding_status" => :dx,
    "audit" => :dx,
    "impact" => :dx,
    "health" => :dx,
    "diff" => :dx,
    "batch" => :dx,
    "semantic_clones" => :dx,
    "architectural_drift" => :dx,
    "bidi_trace" => :dx,
    "api_break_check" => :dx,
    "simulate_mutation" => :dx,
    "ist_snapshot_warm" => :graph,
    "ist_centrality_pagerank" => :graph,
    "ist_structural_sccs" => :graph,
    "ist_shortest_path" => :graph
  }

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      :timer.send_interval(@refresh_ms, self(), :refresh)
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")
      send(self(), :load)
    end

    socket =
      socket
      |> assign(:page_title, "Axon · MCP Catalog")
      |> assign(:tools, [])
      |> assign(:descriptions, %{})
      |> assign(:filter, "")
      |> assign(:category, :all)
      |> assign(:heartbeat, IndexerHeartbeat.latest())
      |> assign(:loaded?, false)
      |> assign(:error, nil)

    {:ok, socket}
  end

  @impl true
  def handle_info(:load, socket), do: {:noreply, load_tools(socket)}

  @impl true
  def handle_info(:refresh, socket), do: {:noreply, load_tools(socket)}

  @impl true
  def handle_info({:indexer_heartbeat, snap}, socket) do
    {:noreply, assign(socket, :heartbeat, snap)}
  end

  @impl true
  def handle_info({:tool_descriptions, map}, socket) do
    {:noreply, assign(socket, :descriptions, Map.merge(socket.assigns.descriptions, map))}
  end

  @impl true
  def handle_info({:tools_loaded, tools}, socket) do
    {:noreply,
     socket
     |> assign(:tools, tools)
     |> assign(:loaded?, true)
     |> assign(:error, nil)}
  end

  @impl true
  def handle_info({:tools_error, reason}, socket) do
    {:noreply,
     socket
     |> assign(:error, reason)
     |> assign(:loaded?, true)}
  end

  @impl true
  def handle_info(_, socket), do: {:noreply, socket}

  @impl true
  def handle_event("filter", %{"value" => v}, socket) do
    {:noreply, assign(socket, :filter, String.downcase(v))}
  end

  @impl true
  def handle_event("category", %{"value" => v}, socket) do
    {:noreply, assign(socket, :category, String.to_atom(v))}
  end

  @impl true
  def render(assigns) do
    visible =
      assigns.tools
      |> apply_category(assigns.category)
      |> apply_filter(assigns.filter)
      |> Enum.sort_by(& &1.name)

    grouped = Enum.group_by(visible, & &1.category)
    assigns = assign(assigns, :visible, visible) |> assign(:grouped, grouped)

    ~H"""
    <Nav.shell
      current={:mcp}
      build_id={(@heartbeat || %{}) |> Map.get(:build_id, "n/a")}
      install_generation={(@heartbeat || %{}) |> Map.get(:install_generation, "n/a")}
      runtime_mode={(@heartbeat || %{}) |> Map.get(:runtime_mode, "unknown")}
      instance_kind={System.get_env("AXON_INSTANCE_KIND") || "unknown"}
      gpu_effective={get_in(@heartbeat || %{}, [:embedder_provider, :effective]) || "unknown"}
      degraded_reason={(@heartbeat || %{}) |> Map.get(:degraded_reason)}
      stale={Map.get(@heartbeat || %{}, :stale, false) == true}
      observed_age_ms={(@heartbeat || %{}) |> Map.get(:observed_age_ms)}
    >
      <div class="space-y-4">
        <%!-- HEADER + SEARCH --%>
        <section class="flex items-center gap-4 flex-wrap">
          <div>
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">MCP Catalog</div>
            <h1 class="text-xl font-semibold text-slate-100">{length(@tools)} public tools</h1>
          </div>

          <div class="ml-auto flex items-center gap-2">
            <div class="flex items-center gap-1 bg-slate-900/60 border border-slate-800 rounded-md px-1 py-0.5">
              <.cat_tab current={@category} value={:all} label={"All (#{length(@tools)})"} />
              <.cat_tab current={@category} value={:dx} label={"DX (#{count_cat(@tools, :dx)})"} />
              <.cat_tab current={@category} value={:soll} label={"SOLL (#{count_cat(@tools, :soll)})"} />
              <.cat_tab current={@category} value={:graph} label={"Graph (#{count_cat(@tools, :graph)})"} />
              <.cat_tab current={@category} value={:system} label={"System (#{count_cat(@tools, :system)})"} />
              <.cat_tab current={@category} value={:other} label={"Other (#{count_cat(@tools, :other)})"} />
            </div>

            <input
              type="text"
              phx-keyup="filter"
              phx-debounce="120"
              name="value"
              value={@filter}
              placeholder="filter by name or description…"
              class="bg-slate-900/60 border border-slate-800 rounded-md px-3 py-1.5 text-sm font-mono w-72 text-slate-100 placeholder:text-slate-600 focus:outline-none focus:border-amber-500/60 focus:ring-1 focus:ring-amber-500/20"
            />
          </div>
        </section>

        <%!-- ERROR --%>
        <div :if={@error} class="rounded-md border border-red-500/40 bg-red-950/40 p-4 text-[12px] font-mono text-red-200">
          MCP error: {@error}
        </div>

        <%!-- LOADING --%>
        <div :if={not @loaded? and is_nil(@error)} class="text-slate-500 text-sm font-mono px-4 py-12 text-center">
          Loading MCP tool catalog…
        </div>

        <%!-- GROUPED LIST --%>
        <section :for={{cat, list} <- group_order(@grouped)} class="rounded-xl border border-slate-800 bg-slate-900/60 backdrop-blur-sm overflow-hidden">
          <header class="px-5 py-2.5 border-b border-slate-800 bg-slate-950/40 flex items-center justify-between">
            <div class="flex items-center gap-2">
              <span class={["h-1.5 w-1.5 rounded-full", cat_dot(cat)]}></span>
              <h2 class="text-[11px] uppercase tracking-[0.2em] text-slate-300 font-semibold">{cat_label(cat)}</h2>
              <span class="text-[10px] font-mono text-slate-600">{length(list)} tools</span>
            </div>
          </header>
          <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-px bg-slate-800/40">
            <div
              :for={tool <- list}
              class="bg-slate-900/40 hover:bg-slate-800/40 transition-colors p-3 cursor-pointer group"
              title={Map.get(@descriptions, tool.name) || tool.description}
            >
              <div class="flex items-center justify-between gap-2">
                <code class="font-mono text-[12px] font-semibold text-amber-300 group-hover:text-amber-200 transition-colors truncate">
                  {tool.name}
                </code>
                <span class={["text-[9px] uppercase font-semibold tracking-wider px-1.5 py-0.5 rounded border", cat_chip(tool.category)]}>
                  {cat_short(tool.category)}
                </span>
              </div>
              <p class="mt-1 text-[11px] text-slate-400 line-clamp-2 leading-snug">
                {Map.get(@descriptions, tool.name) || tool.description || "—"}
              </p>
            </div>
          </div>
        </section>

        <%!--
          REQ-AXO-901649 : "No tools match" only when the user has typed a
          filter or picked a non-:all category. An empty filter + :all
          category + 0 tools is "still loading / brain unreachable", not
          a no-match condition — the loading / error blocks above already
          cover that case. Without this guard the page falsely claims
          "No tools match filter """ when the catalog hasn't streamed in
          yet, regressing the cockpit's first-paint contract.
        --%>
        <div
          :if={@loaded? and @visible == [] and (@filter != "" or @category != :all)}
          class="text-slate-500 text-sm font-mono px-4 py-12 text-center"
        >
          No tools match filter <code class="text-slate-300">"{@filter}"</code>.
        </div>
      </div>
    </Nav.shell>
    """
  end

  ## Components

  attr :current, :atom, required: true
  attr :value, :atom, required: true
  attr :label, :string, required: true

  defp cat_tab(assigns) do
    ~H"""
    <button
      phx-click="category"
      phx-value-value={Atom.to_string(@value)}
      class={[
        "px-2.5 py-1 rounded text-[10px] font-mono uppercase tracking-wider transition-colors cursor-pointer",
        if(@current == @value,
          do: "bg-amber-500/20 text-amber-200 border border-amber-500/30",
          else: "text-slate-400 hover:text-slate-200 border border-transparent"
        )
      ]}
    >
      {@label}
    </button>
    """
  end

  ## Data

  defp load_tools(socket) do
    parent = self()

    Task.start(fn ->
      case fetch_tools() do
        {:ok, list} when is_list(list) ->
          tools =
            list
            |> Enum.map(&normalize_tool/1)

          send(parent, {:tools_loaded, tools})

        {:error, reason} ->
          send(parent, {:tools_error, inspect(reason)})
      end
    end)

    socket
  end

  # REQ-AXO-901649 — feature tests stub the catalog via a JSON fixture so
  # the suite never depends on a live brain. The fixture path is given
  # through `AXON_MCP_FIXTURE_PATH` and must point to a file containing a
  # JSON array of `%{"name" => ..., "description" => ...}` objects. Any
  # other env (dev / live / prod) falls through to McpClient.
  defp fetch_tools do
    case System.get_env("AXON_MCP_FIXTURE_PATH") do
      nil ->
        McpClient.list_tools()

      "" ->
        McpClient.list_tools()

      path ->
        with {:ok, body} <- File.read(path),
             {:ok, list} when is_list(list) <- Jason.decode(body) do
          {:ok, list}
        else
          {:ok, other} -> {:error, {:bad_fixture_shape, other}}
          err -> err
        end
    end
  end

  defp normalize_tool(%{"name" => name} = t) do
    desc = Map.get(t, "description", "")

    %{
      name: name,
      description: desc |> String.split("\n", parts: 2) |> List.first() |> trim_to(160),
      category: Map.get(@categories, name, :other),
      input_schema: Map.get(t, "inputSchema", %{})
    }
  end

  defp trim_to(s, n) when byte_size(s) > n, do: String.slice(s, 0, n) <> "…"
  defp trim_to(s, _), do: s

  ## Filters

  defp apply_category(tools, :all), do: tools
  defp apply_category(tools, cat), do: Enum.filter(tools, &(&1.category == cat))

  defp apply_filter(tools, ""), do: tools

  defp apply_filter(tools, query) do
    Enum.filter(tools, fn t ->
      String.contains?(String.downcase(t.name), query) or
        String.contains?(String.downcase(t.description || ""), query)
    end)
  end

  defp count_cat(tools, cat), do: Enum.count(tools, &(&1.category == cat))

  defp group_order(grouped) do
    Enum.flat_map([:dx, :soll, :graph, :system, :other], fn cat ->
      case Map.get(grouped, cat) do
        nil -> []
        [] -> []
        list -> [{cat, list}]
      end
    end)
  end

  defp cat_label(:dx), do: "DX · structural intelligence"
  defp cat_label(:soll), do: "SOLL · intent graph"
  defp cat_label(:graph), do: "IST · graph algorithms"
  defp cat_label(:system), do: "System · meta"
  defp cat_label(:other), do: "Other"
  defp cat_label(_), do: "—"

  defp cat_short(:dx), do: "DX"
  defp cat_short(:soll), do: "SOLL"
  defp cat_short(:graph), do: "IST"
  defp cat_short(:system), do: "SYS"
  defp cat_short(_), do: "?"

  defp cat_chip(:dx), do: "border-cyan-500/30 bg-cyan-500/10 text-cyan-200"
  defp cat_chip(:soll), do: "border-violet-500/30 bg-violet-500/10 text-violet-200"
  defp cat_chip(:graph), do: "border-pink-500/30 bg-pink-500/10 text-pink-200"
  defp cat_chip(:system), do: "border-slate-600/40 bg-slate-700/30 text-slate-300"
  defp cat_chip(_), do: "border-slate-700/40 bg-slate-800/40 text-slate-400"

  defp cat_dot(:dx), do: "bg-cyan-400"
  defp cat_dot(:soll), do: "bg-violet-400"
  defp cat_dot(:graph), do: "bg-pink-400"
  defp cat_dot(:system), do: "bg-slate-400"
  defp cat_dot(_), do: "bg-slate-600"
end
