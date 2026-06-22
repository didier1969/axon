defmodule AxonDashboardWeb.Live.Nav do
  @moduledoc """
  Shared chrome for the three cockpit pages (Pipeline / Projects / MCP).

  REQ-AXO-901647: replaces the old single-page Cockpit shell. Uses Tailwind
  utility classes inline so it doesn't depend on the legacy app.css ruleset
  that targeted obsolete schema fields.
  """
  use Phoenix.Component

  attr :current, :atom, required: true, doc: ":pipeline | :projects | :mcp"
  attr :build_id, :string, default: "n/a"
  attr :install_generation, :string, default: "n/a"
  attr :runtime_mode, :string, default: "unknown"
  attr :instance_kind, :string, default: "unknown"
  attr :gpu_effective, :string, default: "unknown"
  attr :degraded_reason, :string, default: nil
  attr :runtime_idle, :boolean, default: false
  attr :stale, :boolean, default: false
  attr :observed_age_ms, :integer, default: nil
  slot :inner_block, required: true

  def shell(assigns) do
    ~H"""
    <div class="min-h-screen bg-slate-950 text-slate-100 font-sans flex flex-col">
      <header class="sticky top-0 z-30 border-b border-slate-800/80 bg-slate-950/85 backdrop-blur-md">
        <div class="max-w-[1600px] mx-auto px-6 py-3 flex items-center gap-6">
          <div class="flex items-center gap-2">
            <div class="h-8 w-8 rounded-md bg-gradient-to-br from-amber-500 to-amber-700 grid place-items-center font-mono font-bold text-slate-950">
              AX
            </div>
            <div class="leading-tight">
              <div class="text-sm font-semibold tracking-wide">Axon Cockpit</div>
              <div class="text-[10px] uppercase tracking-[0.18em] text-slate-500">
                Structural Intelligence
              </div>
            </div>
          </div>

          <nav class="flex items-center gap-1 ml-2">
            <.nav_link href="/" current?={@current == :pipeline} label="Pipeline" />
            <.nav_link href="/projects" current?={@current == :projects} label="Projects" />
            <.nav_link href="/mcp" current?={@current == :mcp} label="MCP" />
          </nav>

          <% {h_tone, h_label, h_detail} = health(@stale, @observed_age_ms, @degraded_reason, @runtime_idle) %>
          <div class="ml-auto flex items-center gap-2 text-[11px] font-mono">
            <.badge value={h_label} tone={h_tone} dot={true} />
            <.badge label="instance" value={@instance_kind} tone={:neutral} />
            <.badge label="mode" value={@runtime_mode} tone={:neutral} />
            <.badge label="gpu" value={@gpu_effective} tone={gpu_tone(@gpu_effective)} />
            <.badge label="build" value={short_build(@build_id)} tone={:neutral} />
          </div>
        </div>
        <%!-- REQ-AXO-901856 — honest status line. The banner appears ONLY for a
             real problem (stale envelope or a non-heartbeat degraded reason). A
             fresh "missing_runtime_truth_heartbeat" while idle is no longer a red
             alarm: the health pill reads IDLE and the footer carries the age. --%>
        <div :if={h_tone in [:warn, :danger]} class={[
          "border-t text-[11px] font-mono",
          if(h_tone == :danger, do: "bg-red-950/40 border-red-900/60", else: "bg-amber-950/40 border-amber-900/60")
        ]}>
          <div class={[
            "max-w-[1600px] mx-auto px-6 py-1.5 flex items-center gap-2",
            if(h_tone == :danger, do: "text-red-300/90", else: "text-amber-300/90")
          ]}>
            <span class={[
              "inline-block h-1.5 w-1.5 rounded-full animate-pulse",
              if(h_tone == :danger, do: "bg-red-400", else: "bg-amber-400")
            ]}></span>
            {h_label}<span :if={h_detail}>: {h_detail}</span>
          </div>
        </div>
      </header>

      <main class="flex-1 max-w-[1600px] w-full mx-auto px-6 py-6">
        {render_slot(@inner_block)}
      </main>

      <footer class="border-t border-slate-800/80 bg-slate-950/70 mt-6">
        <div class="max-w-[1600px] mx-auto px-6 py-2 flex items-center gap-4 text-[10px] font-mono text-slate-500 uppercase tracking-wider">
          <span>install <span class="text-slate-300">{@install_generation}</span></span>
          <span>heartbeat age <span class={if @stale, do: "text-amber-400", else: "text-emerald-400"}>{age_label(@observed_age_ms, @stale)}</span></span>
          <span class="ml-auto text-slate-600">REQ-AXO-901647 · pipeline-v2 cockpit</span>
        </div>
      </footer>
    </div>
    """
  end

  attr :href, :string, required: true
  attr :current?, :boolean, required: true
  attr :label, :string, required: true

  defp nav_link(assigns) do
    ~H"""
    <.link
      navigate={@href}
      class={[
        "px-3 py-1.5 rounded-md text-xs font-medium tracking-wide transition-colors",
        if(@current?,
          do: "bg-amber-500/15 text-amber-300 border border-amber-500/30",
          else: "text-slate-400 hover:text-slate-200 hover:bg-slate-800/60 border border-transparent"
        )
      ]}
    >
      {@label}
    </.link>
    """
  end

  @doc """
  REQ-AXO-901856 — single inline badge primitive shared across the whole
  cockpit (header chips, stage tags, project tags). `label` is an optional
  uppercase eyebrow; `value` the strong text; `tone` drives the one canonical
  palette (ok/warn/danger/info/neutral); `dot` prepends a status dot.
  """
  attr :label, :string, default: nil
  attr :value, :string, required: true
  attr :tone, :atom, default: :neutral
  attr :dot, :boolean, default: false

  def badge(assigns) do
    ~H"""
    <span class={[
      "inline-flex items-center gap-1.5 px-2 py-1 rounded-md border text-[10px] whitespace-nowrap",
      badge_tone_class(@tone)
    ]}>
      <span :if={@dot} class={["h-1.5 w-1.5 rounded-full", badge_dot_class(@tone)]}></span>
      <span :if={@label} class="uppercase tracking-[0.14em] text-slate-500">{@label}</span>
      <strong class="font-semibold tracking-wide uppercase">{@value}</strong>
    </span>
    """
  end

  @doc "Canonical tone → border/bg/text classes for every cockpit badge."
  def badge_tone_class(:ok), do: "border-emerald-500/30 bg-emerald-500/5 text-emerald-200"
  def badge_tone_class(:warn), do: "border-amber-500/40 bg-amber-500/10 text-amber-200"
  def badge_tone_class(:danger), do: "border-red-500/40 bg-red-500/10 text-red-200"
  def badge_tone_class(:info), do: "border-cyan-500/30 bg-cyan-500/5 text-cyan-200"
  def badge_tone_class(_), do: "border-slate-700/60 bg-slate-800/40 text-slate-300"

  defp badge_dot_class(:ok), do: "bg-emerald-400"
  defp badge_dot_class(:warn), do: "bg-amber-400"
  defp badge_dot_class(:danger), do: "bg-red-400"
  defp badge_dot_class(:info), do: "bg-cyan-400"
  defp badge_dot_class(_), do: "bg-slate-400"

  # REQ-AXO-901856 — derive one honest health verdict from observed truth.
  # A fresh envelope (not stale) whose only complaint is a missing telemetry
  # heartbeat is NOT degraded — it is an idle/quiet system, so it reads INFO,
  # never a red DEGRADED alarm. Order: stale > hard-degraded > idle > live.
  defp health(stale, observed_age_ms, degraded_reason, runtime_idle) do
    cond do
      stale or (is_integer(observed_age_ms) and observed_age_ms > 10_000) ->
        {:danger, "STALE", age_label(observed_age_ms, true)}

      is_binary(degraded_reason) and not heartbeat_reason?(degraded_reason) ->
        {:warn, "DEGRADED", degraded_reason}

      runtime_idle ->
        {:info, "IDLE", "quiet cruise"}

      true ->
        {:ok, "LIVE", nil}
    end
  end

  defp heartbeat_reason?(reason), do: String.contains?(reason, "heartbeat")

  defp gpu_tone("cuda"), do: :ok
  defp gpu_tone("tensorrt"), do: :ok
  defp gpu_tone("cpu"), do: :warn
  defp gpu_tone(_), do: :neutral

  defp short_build(nil), do: "n/a"
  defp short_build(build_id) when is_binary(build_id) do
    case String.split(build_id, "-g") do
      [_, sha] -> "g" <> String.slice(sha, 0, 7)
      _ -> String.slice(build_id, 0, 12)
    end
  end
  defp short_build(_), do: "n/a"

  defp age_label(nil, _stale), do: "—"
  defp age_label(age_ms, true), do: "stale (#{format_age(age_ms)})"
  defp age_label(age_ms, _), do: format_age(age_ms)

  defp format_age(ms) when is_integer(ms) and ms < 1000, do: "#{ms}ms"
  defp format_age(ms) when is_integer(ms) and ms < 60_000, do: "#{div(ms, 1000)}s"
  defp format_age(ms) when is_integer(ms), do: "#{div(ms, 60_000)}m"
  defp format_age(_), do: "—"
end
