defmodule AxonDashboardWeb.Live.WiringLive do
  @moduledoc """
  REQ-AXO-902192 slice S4 (minimal) — wiring / orphan lineage view (non-canonical,
  PIL-AXO-009).

  Reads the brain MCP tool `wiring` (`data.orphans[]` + counts) and renders the
  per-project list of structurally-orphaned symbols: public production symbols
  with no production caller (`isolated`) or only test callers (`test_only`). A
  project selector re-scopes the query server-side.

  The analytic engine (`wiring` / `orphan_clusters` + the CI gate) is the
  delivered half of the umbrella (REQ-AXO-902211/224/225/227); this is the
  visualisation half. Server-as-source-of-truth: pulled inside a supervised Task,
  refreshed on a server-driven interval — no client fetch / setInterval.
  """
  use Phoenix.LiveView

  alias AxonDashboard.{BridgeClient, DashboardState}
  alias Axon.Watcher.McpClient
  alias AxonDashboardWeb.Live.Nav

  @refresh_ms 30_000
  @default_project "AXO"

  # Projects with a meaningful indexed footprint, for the selector. Kept static
  # (the registry is stable); a stale entry just yields an empty orphan list.
  @projects ~w(
    AGO APS AXO BKS CDV CHC CSC CTX DOC DSD ERP EXA FLA FSF HYC INK LLL MFL
    MLD MSC NEX NTO ODM OLL OPT OPV PLC PRP ROM SNN SOK SVZ SWX TE2 TNT TRD
    TRI UNU VPC XON
  )

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      :timer.send_interval(@refresh_ms, self(), :refresh)
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, BridgeClient.dashboard_topic())
      send(self(), :load)
    end

    socket =
      socket
      |> assign(:page_title, "Axon · Wiring")
      |> assign(:project, @default_project)
      |> assign(:projects, @projects)
      |> assign(:orphans, [])
      |> assign(:test_only_count, 0)
      |> assign(:isolated_count, 0)
      |> assign(:soll_declared, 0)
      |> assign(:loaded?, false)
      |> assign(:error, nil)
      |> assign(:dashboard_state, BridgeClient.dashboard_state() || %DashboardState{})

    {:ok, socket}
  end

  @impl true
  def handle_event("select_project", %{"project" => project}, socket) do
    project = if project in @projects, do: project, else: socket.assigns.project

    {:noreply,
     socket
     |> assign(:project, project)
     |> assign(:loaded?, false)
     |> assign(:error, nil)
     |> load_wiring()}
  end

  @impl true
  def handle_info(:load, socket), do: {:noreply, load_wiring(socket)}

  @impl true
  def handle_info(:refresh, socket), do: {:noreply, load_wiring(socket)}

  @impl true
  def handle_info({:wiring_loaded, project, orphans, counts}, socket) do
    # Ignore a stale in-flight result if the operator switched project meanwhile.
    if project == socket.assigns.project do
      {:noreply,
       socket
       |> assign(:orphans, orphans)
       |> assign(:test_only_count, counts.test_only)
       |> assign(:isolated_count, counts.isolated)
       |> assign(:soll_declared, counts.soll_declared)
       |> assign(:loaded?, true)
       |> assign(:error, nil)}
    else
      {:noreply, socket}
    end
  end

  @impl true
  def handle_info({:wiring_error, project, reason}, socket) do
    if project == socket.assigns.project do
      {:noreply, socket |> assign(:loaded?, true) |> assign(:error, reason)}
    else
      {:noreply, socket}
    end
  end

  @impl true
  def handle_info({:dashboard_state, %DashboardState{} = state}, socket) do
    {:noreply, assign(socket, :dashboard_state, state)}
  end

  @impl true
  def handle_info(_, socket), do: {:noreply, socket}

  @impl true
  def render(assigns) do
    ~H"""
    <Nav.shell
      current={:wiring}
      build_id={runtime_field(@dashboard_state, :build_id, "n/a")}
      install_generation={runtime_field(@dashboard_state, :install_generation, "n/a")}
      runtime_mode={runtime_field(@dashboard_state, :runtime_mode, "unknown")}
      instance_kind={runtime_field(@dashboard_state, :instance_kind, Application.get_env(:axon_dashboard, :instance_kind, "unknown"))}
      gpu_effective={embedder_field(@dashboard_state, :effective, "unknown")}
      degraded_reason={runtime_field(@dashboard_state, :degraded_reason, nil)}
      stale={is_nil(@dashboard_state.ts_ms)}
      observed_age_ms={DashboardState.observed_age_ms(@dashboard_state)}
    >
      <div class="space-y-4">
        <%!-- HEADER --%>
        <section class="flex items-center gap-4 flex-wrap">
          <div>
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Wiring · Orphaned symbols</div>
            <h1 class="text-xl font-semibold text-slate-100">
              {length(@orphans)} orphan(s) · {@soll_declared} SOLL-declared
            </h1>
          </div>
          <div class="ml-auto flex items-center gap-2">
            <form phx-change="select_project">
              <select
                name="project"
                class="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-[12px] font-mono text-slate-200 focus:border-amber-400 focus:outline-none"
              >
                <option :for={p <- @projects} value={p} selected={p == @project}>{p}</option>
              </select>
            </form>
            <Nav.badge label="test-only" value={Integer.to_string(@test_only_count)} tone={if @test_only_count == 0, do: :ok, else: :warn} dot={true} />
            <Nav.badge label="isolated" value={Integer.to_string(@isolated_count)} tone={if @isolated_count == 0, do: :ok, else: :danger} dot={true} />
          </div>
        </section>

        <%!-- ERROR --%>
        <div :if={@error} class="rounded-md border border-red-500/40 bg-red-950/40 p-4 text-[12px] font-mono text-red-200">
          wiring error: {@error}
        </div>

        <%!-- LOADING --%>
        <div :if={not @loaded? and is_nil(@error)} class="rounded-md border border-slate-800 bg-slate-900/40 p-6 text-sm font-mono text-slate-500">
          Loading wiring for {@project}…
        </div>

        <%!-- EMPTY (this is the healthy state) --%>
        <div
          :if={@loaded? and is_nil(@error) and @orphans == []}
          class="rounded-md border border-emerald-800/50 bg-emerald-950/20 p-6 text-sm font-mono text-emerald-300"
        >
          ✓ No orphaned public symbols in {@project}. Every public production symbol has a production caller.
        </div>

        <%!-- ORPHAN TABLE --%>
        <section :if={@orphans != []} class="overflow-x-auto rounded-lg border border-slate-800 bg-slate-900/40">
          <table class="min-w-full text-[12px] font-mono">
            <thead>
              <tr class="text-left text-slate-500 uppercase tracking-[0.14em] text-[10px]">
                <th class="px-3 py-2">Category</th>
                <th class="px-3 py-2">Kind</th>
                <th class="px-3 py-2">Symbol</th>
                <th class="px-3 py-2 text-right">Test callers</th>
                <th class="px-3 py-2">Id</th>
              </tr>
            </thead>
            <tbody>
              <tr :for={o <- @orphans} class="border-t border-slate-800/60 hover:bg-slate-800/30">
                <td class="px-3 py-1.5">
                  <span class={["rounded-sm px-1.5 py-0.5 text-[10px] uppercase tracking-wide", category_tone(o.category)]}>
                    {o.category}
                  </span>
                </td>
                <td class="px-3 py-1.5 text-slate-400">{o.kind}</td>
                <td class="px-3 py-1.5 text-slate-100">{o.name}</td>
                <td class="px-3 py-1.5 text-right tabular-nums text-slate-400">{o.test_callers}</td>
                <td class="px-3 py-1.5 text-slate-500 truncate max-w-[28rem]" title={o.id}>{short_id(o.id)}</td>
              </tr>
            </tbody>
          </table>
        </section>

        <%!-- LEGEND --%>
        <section :if={@orphans != []} class="flex items-center gap-4 text-[10px] font-mono text-slate-500">
          <span class="flex items-center gap-1">
            <span class="h-3 w-3 rounded-sm bg-red-500/40"></span>isolated — no caller at all
          </span>
          <span class="flex items-center gap-1">
            <span class="h-3 w-3 rounded-sm bg-amber-500/40"></span>test_only — only test callers (prod-dead)
          </span>
        </section>
      </div>
    </Nav.shell>
    """
  end

  ## Data

  defp load_wiring(socket) do
    parent = self()
    project = socket.assigns.project

    Task.Supervisor.start_child(AxonDashboard.TaskSupervisor, fn ->
      case McpClient.call_tool("wiring", %{"project_code" => project}) do
        {:ok, result} ->
          orphans =
            result
            |> extract_field("orphans", [])
            |> Enum.map(&normalize_orphan/1)
            |> Enum.reject(&is_nil/1)

          counts = %{
            test_only: extract_field(result, "test_only_count", 0) |> to_int(),
            isolated: extract_field(result, "isolated_count", 0) |> to_int(),
            soll_declared: extract_field(result, "soll_declared_symbols", 0) |> to_int()
          }

          send(parent, {:wiring_loaded, project, orphans, counts})

        {:error, reason} ->
          send(parent, {:wiring_error, project, inspect(reason)})
      end
    end)

    socket
  end

  # The brain surfaces tool data either as `structuredContent` (→ `_structured`
  # by McpClient) or as the raw `data` envelope; tolerate both.
  defp extract_field(result, key, default) when is_map(result) do
    cond do
      not is_nil(get_in(result, ["_structured", key])) -> get_in(result, ["_structured", key])
      not is_nil(get_in(result, ["data", key])) -> get_in(result, ["data", key])
      not is_nil(Map.get(result, key)) -> Map.get(result, key)
      true -> default
    end
  end

  defp extract_field(_, _key, default), do: default

  defp normalize_orphan(%{"id" => id} = o) do
    %{
      id: to_string(id),
      name: to_string(Map.get(o, "name", "")),
      kind: to_string(Map.get(o, "kind", "")),
      category: to_string(Map.get(o, "category", "")),
      test_callers: to_int(Map.get(o, "test_callers", 0))
    }
  end

  defp normalize_orphan(_), do: nil

  defp to_int(n) when is_integer(n), do: n
  defp to_int(n) when is_float(n), do: trunc(n)

  defp to_int(s) when is_binary(s) do
    case Integer.parse(s) do
      {i, _} -> i
      :error -> 0
    end
  end

  defp to_int(_), do: 0

  defp category_tone("isolated"), do: "bg-red-500/40 text-red-100"
  defp category_tone("test_only"), do: "bg-amber-500/40 text-amber-100"
  defp category_tone(_), do: "bg-slate-700/40 text-slate-300"

  # Ids are `PROJ::path::to::file.rs::symbol` — show the last two segments.
  defp short_id(id) when is_binary(id) do
    case String.split(id, "::") do
      parts when length(parts) >= 2 -> parts |> Enum.take(-2) |> Enum.join("::")
      _ -> id
    end
  end

  defp short_id(_), do: "—"

  ## DashboardState accessors (REQ-AXO-901826) — typed struct, atom keys.
  defp runtime_field(%DashboardState{runtime: nil}, _key, default), do: default
  defp runtime_field(%DashboardState{runtime: r}, key, default), do: Map.get(r, key, default) || default

  defp embedder_field(%DashboardState{embedder: nil}, _key, default), do: default
  defp embedder_field(%DashboardState{embedder: e}, key, default), do: Map.get(e, key, default) || default
end
