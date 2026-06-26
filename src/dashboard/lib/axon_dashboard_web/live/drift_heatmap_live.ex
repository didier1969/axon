defmodule AxonDashboardWeb.Live.DriftHeatmapLive do
  @moduledoc """
  REQ-AXO-902068 — architectural-drift heatmap (non-canonical view, PIL-AXO-009).

  Reads `ist.drift_history` through the brain MCP tool `drift_history`
  (action=read) and renders a 2D heatmap: one row per `layer_pair`, one column
  per recorded wave (`wave_ts`), each cell coloured by the EWMA drift score with
  an alert marker when `alert == true`. The engine (EWMA + table + tool) is the
  delivered half (REQ-AXO-158, commit 0f76f7b6); this is the visualisation half.

  Server-as-source-of-truth: the data is pulled inside a supervised Task and the
  view refreshes on a server-driven interval — no client fetch / setInterval.
  """
  use Phoenix.LiveView

  alias AxonDashboard.{BridgeClient, DashboardState}
  alias Axon.Watcher.McpClient
  alias AxonDashboardWeb.Live.Nav

  @refresh_ms 30_000
  @project "AXO"
  # Cap the time-axis so the grid stays legible; newest waves on the right.
  @max_waves 24
  @read_limit 400

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      :timer.send_interval(@refresh_ms, self(), :refresh)
      Phoenix.PubSub.subscribe(AxonDashboard.PubSub, BridgeClient.dashboard_topic())
      send(self(), :load)
    end

    socket =
      socket
      |> assign(:page_title, "Axon · Drift Heatmap")
      |> assign(:project_label, @project)
      |> assign(:samples, [])
      |> assign(:alerts, 0)
      |> assign(:loaded?, false)
      |> assign(:error, nil)
      |> assign(:dashboard_state, BridgeClient.dashboard_state() || %DashboardState{})

    {:ok, socket}
  end

  @impl true
  def handle_info(:load, socket), do: {:noreply, load_drift(socket)}

  @impl true
  def handle_info(:refresh, socket), do: {:noreply, load_drift(socket)}

  @impl true
  def handle_info({:drift_loaded, samples, alerts}, socket) do
    {:noreply,
     socket
     |> assign(:samples, samples)
     |> assign(:alerts, alerts)
     |> assign(:loaded?, true)
     |> assign(:error, nil)}
  end

  @impl true
  def handle_info({:drift_error, reason}, socket) do
    {:noreply,
     socket
     |> assign(:loaded?, true)
     |> assign(:error, reason)}
  end

  @impl true
  def handle_info({:dashboard_state, %DashboardState{} = state}, socket) do
    {:noreply, assign(socket, :dashboard_state, state)}
  end

  @impl true
  def handle_info(_, socket), do: {:noreply, socket}

  @impl true
  def render(assigns) do
    layer_pairs = assigns.samples |> Enum.map(& &1.layer_pair) |> Enum.uniq() |> Enum.sort()

    waves =
      assigns.samples
      |> Enum.map(& &1.wave_ts)
      |> Enum.uniq()
      |> Enum.sort()
      |> Enum.take(-@max_waves)

    cells = Map.new(assigns.samples, fn s -> {{s.layer_pair, s.wave_ts}, s} end)

    assigns =
      assigns
      |> assign(:layer_pairs, layer_pairs)
      |> assign(:waves, waves)
      |> assign(:cells, cells)

    ~H"""
    <Nav.shell
      current={:drift}
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
            <div class="text-[10px] uppercase tracking-[0.18em] text-amber-400/80">Architectural Drift</div>
            <h1 class="text-xl font-semibold text-slate-100">
              {length(@layer_pairs)} layer pair(s) · {length(@waves)} wave(s)
            </h1>
          </div>
          <div class="ml-auto flex items-center gap-2">
            <Nav.badge label="alerts" value={Integer.to_string(@alerts)} tone={if @alerts == 0, do: :ok, else: :danger} dot={true} />
            <Nav.badge label="project" value={@project_label} tone={:neutral} />
          </div>
        </section>

        <%!-- ERROR --%>
        <div :if={@error} class="rounded-md border border-red-500/40 bg-red-950/40 p-4 text-[12px] font-mono text-red-200">
          drift_history error: {@error}
        </div>

        <%!-- LOADING --%>
        <div :if={not @loaded? and is_nil(@error)} class="rounded-md border border-slate-800 bg-slate-900/40 p-6 text-sm font-mono text-slate-500">
          Loading drift history…
        </div>

        <%!-- EMPTY --%>
        <div
          :if={@loaded? and is_nil(@error) and @layer_pairs == []}
          class="rounded-md border border-slate-800 bg-slate-900/40 p-6 text-sm font-mono text-slate-400"
        >
          No drift history recorded yet. Run <code class="text-amber-300">drift_history action=record project={@project_label}</code>
          (after warming the IST snapshot) to capture the first wave.
        </div>

        <%!-- HEATMAP --%>
        <section :if={@layer_pairs != []} class="overflow-x-auto rounded-lg border border-slate-800 bg-slate-900/40">
          <table class="border-separate border-spacing-1 text-[11px] font-mono">
            <thead>
              <tr>
                <th class="sticky left-0 z-10 bg-slate-900/80 px-3 py-2 text-left text-slate-500 uppercase tracking-[0.14em] text-[10px]">
                  Layer pair
                </th>
                <th :for={wave <- @waves} class="px-2 py-2 text-slate-500 font-normal whitespace-nowrap" title={wave}>
                  {wave_label(wave)}
                </th>
              </tr>
            </thead>
            <tbody>
              <tr :for={lp <- @layer_pairs}>
                <td class="sticky left-0 z-10 bg-slate-900/80 px-3 py-1.5 text-slate-300 whitespace-nowrap">
                  {lp}
                </td>
                <td :for={wave <- @waves} class="p-0">
                  <% cell = @cells[{lp, wave}] %>
                  <div
                    :if={cell}
                    class={[
                      "h-7 w-12 grid place-items-center rounded-sm tabular-nums",
                      cell_tone(cell.ewma),
                      cell.alert && "ring-2 ring-red-400 ring-offset-1 ring-offset-slate-950 animate-pulse"
                    ]}
                    title={"#{lp} · #{wave} · score=#{cell.score} · ewma=#{fmt(cell.ewma)}#{if cell.alert, do: " · ALERT", else: ""}"}
                  >
                    {fmt(cell.ewma)}
                  </div>
                  <div :if={is_nil(cell)} class="h-7 w-12 rounded-sm bg-slate-800/20"></div>
                </td>
              </tr>
            </tbody>
          </table>
        </section>

        <%!-- LEGEND --%>
        <section :if={@layer_pairs != []} class="flex items-center gap-3 text-[10px] font-mono text-slate-500">
          <span class="uppercase tracking-[0.14em]">EWMA drift</span>
          <span class="flex items-center gap-1"><span class="h-3 w-3 rounded-sm bg-emerald-500/30"></span>low</span>
          <span class="flex items-center gap-1"><span class="h-3 w-3 rounded-sm bg-yellow-500/30"></span>mild</span>
          <span class="flex items-center gap-1"><span class="h-3 w-3 rounded-sm bg-amber-500/60"></span>elevated</span>
          <span class="flex items-center gap-1"><span class="h-3 w-3 rounded-sm bg-red-500/70"></span>high</span>
          <span class="flex items-center gap-1"><span class="h-3 w-3 rounded-sm ring-2 ring-red-400"></span>alert</span>
        </section>
      </div>
    </Nav.shell>
    """
  end

  ## Data

  defp load_drift(socket) do
    parent = self()

    Task.Supervisor.start_child(AxonDashboard.TaskSupervisor, fn ->
      case McpClient.call_tool("drift_history", %{
             "project" => @project,
             "action" => "read",
             "limit" => @read_limit
           }) do
        {:ok, result} ->
          samples =
            result
            |> extract_samples()
            |> Enum.map(&normalize_sample/1)
            |> Enum.reject(&is_nil/1)

          alerts = Enum.count(samples, & &1.alert)
          send(parent, {:drift_loaded, samples, alerts})

        {:error, reason} ->
          send(parent, {:drift_error, inspect(reason)})
      end
    end)

    socket
  end

  # The brain surfaces tool data either as `structuredContent` (→ `_structured`
  # by McpClient) or as the raw `data` envelope; tolerate both.
  defp extract_samples(result) when is_map(result) do
    cond do
      is_list(get_in(result, ["_structured", "samples"])) -> result["_structured"]["samples"]
      is_list(get_in(result, ["data", "samples"])) -> result["data"]["samples"]
      true -> []
    end
  end

  defp extract_samples(_), do: []

  # Each sample row is `[layer_pair, wave_ts, score, ewma, alert]`.
  defp normalize_sample([lp, wave, score, ewma, alert | _]) do
    %{
      layer_pair: to_string(lp),
      wave_ts: to_string(wave),
      score: to_number(score),
      ewma: to_number(ewma),
      alert: truthy(alert)
    }
  end

  defp normalize_sample(_), do: nil

  defp to_number(n) when is_number(n), do: n
  defp to_number(s) when is_binary(s) do
    case Float.parse(s) do
      {f, _} -> f
      :error -> 0
    end
  end

  defp to_number(_), do: 0

  defp truthy(true), do: true
  defp truthy("true"), do: true
  defp truthy(1), do: true
  defp truthy(_), do: false

  # Higher EWMA = worse drift. Thresholds mirror the Z-score scale (DEC-AXO-901650).
  defp cell_tone(ewma) when is_number(ewma) do
    cond do
      ewma >= 3.0 -> "bg-red-500/70 text-red-50"
      ewma >= 1.5 -> "bg-amber-500/60 text-amber-50"
      ewma >= 0.5 -> "bg-yellow-500/30 text-yellow-100"
      ewma > 0.0 -> "bg-emerald-500/30 text-emerald-100"
      true -> "bg-slate-800/40 text-slate-500"
    end
  end

  defp cell_tone(_), do: "bg-slate-800/40 text-slate-500"

  defp fmt(n) when is_float(n), do: :erlang.float_to_binary(n, decimals: 1)
  defp fmt(n) when is_integer(n), do: Integer.to_string(n)
  defp fmt(_), do: "—"

  # `wave_ts` is a PG timestamptz text like "2026-06-26 12:34:56.789+00";
  # show MM-DD HH:MM for a compact column header.
  defp wave_label(wave) when is_binary(wave) do
    case String.split(wave, " ") do
      [date, time | _] ->
        d = date |> String.slice(5, 5)
        t = time |> String.slice(0, 5)
        "#{d} #{t}"

      _ ->
        String.slice(wave, 0, 11)
    end
  end

  defp wave_label(_), do: "—"

  ## DashboardState accessors (REQ-AXO-901826) — typed struct, atom keys.
  defp runtime_field(%DashboardState{runtime: nil}, _key, default), do: default
  defp runtime_field(%DashboardState{runtime: r}, key, default), do: Map.get(r, key, default) || default

  defp embedder_field(%DashboardState{embedder: nil}, _key, default), do: default
  defp embedder_field(%DashboardState{embedder: e}, key, default), do: Map.get(e, key, default) || default
end
