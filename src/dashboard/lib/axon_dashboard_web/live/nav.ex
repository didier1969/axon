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

          <div class="ml-auto flex items-center gap-2 text-[11px] font-mono">
            <.chip label="mode" value={@runtime_mode} tone={if @stale, do: :warn, else: :ok} />
            <.chip label="instance" value={@instance_kind} tone={:neutral} />
            <.chip label="gpu" value={@gpu_effective} tone={gpu_tone(@gpu_effective)} />
            <.chip label="build" value={short_build(@build_id)} tone={:neutral} />
          </div>
        </div>
        <div :if={@degraded_reason} class="bg-amber-950/40 border-t border-amber-900/60 text-[11px] font-mono">
          <div class="max-w-[1600px] mx-auto px-6 py-1.5 text-amber-300/90 flex items-center gap-2">
            <span class="inline-block h-1.5 w-1.5 rounded-full bg-amber-400 animate-pulse"></span>
            DEGRADED: {@degraded_reason}
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

  attr :label, :string, required: true
  attr :value, :string, required: true
  attr :tone, :atom, default: :neutral

  defp chip(assigns) do
    ~H"""
    <div class={[
      "flex items-center gap-1.5 px-2 py-1 rounded-md border text-[10px]",
      chip_class(@tone)
    ]}>
      <span class="uppercase tracking-[0.14em] text-slate-500">{@label}</span>
      <strong class="font-semibold tracking-wide">{@value}</strong>
    </div>
    """
  end

  defp chip_class(:ok),
    do: "border-emerald-500/30 bg-emerald-500/5 text-emerald-200"

  defp chip_class(:warn),
    do: "border-amber-500/40 bg-amber-500/10 text-amber-200"

  defp chip_class(:danger),
    do: "border-red-500/40 bg-red-500/10 text-red-200"

  defp chip_class(_),
    do: "border-slate-700/60 bg-slate-800/40 text-slate-300"

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
