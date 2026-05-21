defmodule AxonDashboardWeb.NexusComponents do
  @moduledoc """
  Shared "War Room" components for the Nexus v2 dashboard surfaces.

  Used by:
    - `Axon.Watcher.PipelineLive`  (/)
    - `Axon.Watcher.ProjectsLive`  (/projects)
    - `Axon.Watcher.McpLive`       (/mcp)
    - `Axon.Watcher.CockpitLive`   (/legacy) — left untouched for fallback

  Aesthetic DNA (industrial dark, glassmorphism, amber tactical accent) per
  the phoenix-liveview-architect skill.
  """

  use Phoenix.Component

  ## Layout shell

  attr :current_path, :string, required: true
  attr :brain_up, :boolean, default: false
  attr :indexer_fresh, :boolean, default: false
  attr :title, :string, default: "Nexus"
  attr :subtitle, :string, default: nil
  slot :inner_block, required: true
  slot :header_extra

  def shell(assigns) do
    ~H"""
    <div class="min-h-screen bg-slate-950 text-slate-200 antialiased nexus-root">
      <div class="grid grid-cols-12 min-h-screen">
        <aside class="col-span-2 border-r border-slate-800 bg-slate-900/40 backdrop-blur-md p-4 flex flex-col gap-6">
          <div>
            <div class="flex items-center gap-2 mb-1">
              <span class="w-2 h-2 rounded-full bg-amber-500 animate-pulse" aria-hidden="true"></span>
              <span class="text-amber-500 uppercase tracking-[0.2em] text-[10px] font-semibold font-mono">Axon Nexus</span>
            </div>
            <div class="text-[10px] text-slate-500 font-mono">v2 · War Room</div>
          </div>

          <nav class="flex flex-col gap-1 text-xs">
            <.nav_item href="/" current={@current_path} label="Pipeline" icon="flow" />
            <.nav_item href="/projects" current={@current_path} label="Projects" icon="grid" />
            <.nav_item href="/mcp" current={@current_path} label="MCP catalog" icon="terminal" />
            <.nav_item href="/legacy" current={@current_path} label="Legacy cockpit" icon="archive" />
          </nav>

          <div class="mt-auto space-y-2">
            <.status_chip label="Brain" up={@brain_up} />
            <.status_chip label="Indexer" up={@indexer_fresh} />
            <div class="text-[9px] text-slate-600 leading-tight pt-2 border-t border-slate-800 font-mono">
              live · PG 44144 · MCP 44129
            </div>
          </div>
        </aside>

        <main class="col-span-10 flex flex-col min-h-0">
          <header class="border-b border-slate-800 bg-slate-950/80 backdrop-blur-md px-6 py-3 flex items-center gap-4">
            <div class="flex-1">
              <h1 class="text-slate-100 font-semibold tracking-tight font-mono"><%= @title %></h1>
              <%= if @subtitle do %>
                <p class="text-[11px] text-slate-500 font-mono"><%= @subtitle %></p>
              <% end %>
            </div>
            <%= render_slot(@header_extra) %>
            <div class="text-[10px] text-slate-500 uppercase tracking-widest font-mono">
              <span id="nexus-clock" phx-hook="NexusClock" phx-update="ignore"></span>
            </div>
          </header>

          <div class="flex-1 min-h-0 overflow-auto p-6 font-mono">
            <%= render_slot(@inner_block) %>
          </div>
        </main>
      </div>
    </div>
    """
  end

  attr :href, :string, required: true
  attr :current, :string, required: true
  attr :label, :string, required: true
  attr :icon, :string, required: true

  defp nav_item(assigns) do
    active? =
      assigns.current == assigns.href or
        (assigns.href != "/" and String.starts_with?(assigns.current, assigns.href))

    assigns = assign(assigns, :active?, active?)

    ~H"""
    <.link
      navigate={@href}
      class={[
        "flex items-center gap-2 px-2 py-1.5 rounded-md transition-colors duration-150 font-mono cursor-pointer",
        if(@active?,
          do: "bg-amber-500/10 text-amber-300 border border-amber-500/30",
          else: "text-slate-400 hover:bg-slate-800/60 hover:text-slate-100 border border-transparent"
        )
      ]}
    >
      <.nav_icon name={@icon} />
      <span><%= @label %></span>
    </.link>
    """
  end

  attr :name, :string, required: true

  defp nav_icon(%{name: "flow"} = assigns) do
    ~H"""
    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="4" cy="6" r="2" />
      <circle cx="12" cy="12" r="2" />
      <circle cx="20" cy="18" r="2" />
      <path d="M6 6h4M14 12h4" />
    </svg>
    """
  end

  defp nav_icon(%{name: "grid"} = assigns) do
    ~H"""
    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <rect x="3" y="3" width="7" height="7" /><rect x="14" y="3" width="7" height="7" />
      <rect x="3" y="14" width="7" height="7" /><rect x="14" y="14" width="7" height="7" />
    </svg>
    """
  end

  defp nav_icon(%{name: "terminal"} = assigns) do
    ~H"""
    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="2" /><path d="M7 9l3 3-3 3M13 15h4" />
    </svg>
    """
  end

  defp nav_icon(%{name: "archive"} = assigns) do
    ~H"""
    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <rect x="3" y="4" width="18" height="4" rx="1" />
      <path d="M5 8v12h14V8M10 13h4" />
    </svg>
    """
  end

  defp nav_icon(assigns) do
    ~H"""
    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="12" r="3" /></svg>
    """
  end

  attr :label, :string, required: true
  attr :up, :boolean, required: true

  defp status_chip(assigns) do
    ~H"""
    <div class="flex items-center gap-2 px-2 py-1.5 rounded-md bg-slate-900/60 border border-slate-800 font-mono">
      <span class={[
        "w-1.5 h-1.5 rounded-full inline-block",
        if(@up, do: "bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.6)] animate-pulse", else: "bg-rose-500")
      ]} aria-hidden="true"></span>
      <span class="text-[10px] uppercase tracking-widest text-slate-400"><%= @label %></span>
      <span class={["ml-auto text-[10px] font-semibold", if(@up, do: "text-emerald-400", else: "text-rose-400")]}>
        <%= if @up, do: "UP", else: "DOWN" %>
      </span>
    </div>
    """
  end

  ## Reusable panels

  attr :title, :string, default: nil
  attr :class, :string, default: ""
  slot :inner_block, required: true
  slot :actions

  def panel(assigns) do
    ~H"""
    <section class={[
      "bg-slate-900/60 backdrop-blur-md border border-slate-800 rounded-lg",
      "shadow-2xl shadow-amber-500/[0.02] p-4 flex flex-col min-h-0",
      @class
    ]}>
      <%= if @title do %>
        <header class="flex items-center mb-3">
          <h2 class="text-[10px] uppercase tracking-[0.2em] text-amber-500 font-semibold font-mono">
            <%= @title %>
          </h2>
          <span class="flex-1"></span>
          <%= render_slot(@actions) %>
        </header>
      <% end %>
      <div class="flex-1 min-h-0">
        <%= render_slot(@inner_block) %>
      </div>
    </section>
    """
  end

  attr :label, :string, required: true
  attr :value, :any, required: true
  attr :delta, :any, default: nil
  attr :unit, :string, default: nil
  attr :tone, :string, default: "neutral"
  attr :hint, :string, default: nil

  def stat(assigns) do
    ~H"""
    <div class="flex flex-col gap-0.5">
      <span class="text-[9px] uppercase tracking-widest text-slate-500 font-mono"><%= @label %></span>
      <div class="flex items-baseline gap-1">
        <span class={[
          "font-mono text-xl tabular-nums",
          case @tone do
            "good" -> "text-emerald-300"
            "warn" -> "text-amber-300"
            "bad" -> "text-rose-300"
            _ -> "text-slate-100"
          end
        ]}>
          <%= @value %>
        </span>
        <%= if @unit do %>
          <span class="text-[10px] text-slate-500 font-mono"><%= @unit %></span>
        <% end %>
      </div>
      <%= if @delta do %>
        <span class="text-[10px] text-slate-500 tabular-nums font-mono"><%= @delta %></span>
      <% end %>
      <%= if @hint do %>
        <span class="text-[9px] text-slate-600 font-mono leading-tight"><%= @hint %></span>
      <% end %>
    </div>
    """
  end

  @doc "Render a single stage node in the pipeline visualization."
  attr :stage, :map, required: true
  attr :health, :string, default: "ok"

  def stage_node(assigns) do
    ~H"""
    <div class={[
      "stage-node relative rounded-lg border bg-slate-900/70 backdrop-blur-md p-3 min-w-[140px]",
      case @health do
        "bottleneck" -> "border-rose-500/60 shadow-[0_0_30px_-10px_rgba(244,63,94,0.6)]"
        "warn" -> "border-amber-500/60 shadow-[0_0_24px_-12px_rgba(245,158,11,0.5)]"
        "ok" -> "border-slate-700"
        _ -> "border-slate-800"
      end
    ]}>
      <div class="flex items-center gap-1.5 mb-1">
        <span class={[
          "w-1.5 h-1.5 rounded-full inline-block",
          case @health do
            "bottleneck" -> "bg-rose-500 animate-pulse"
            "warn" -> "bg-amber-500 animate-pulse"
            "ok" -> "bg-emerald-500"
            _ -> "bg-slate-500"
          end
        ]} aria-hidden="true"></span>
        <span class="text-[10px] uppercase tracking-widest text-slate-200 font-mono font-semibold">
          <%= @stage.label %>
        </span>
      </div>
      <div class="text-[9px] text-slate-500 mb-2 font-mono"><%= @stage.sublabel %></div>

      <div class="grid grid-cols-2 gap-1.5 text-[10px] font-mono">
        <div class="flex flex-col">
          <span class="text-slate-600 text-[8px] uppercase tracking-wider">workers</span>
          <span class="text-slate-100 tabular-nums"><%= @stage.workers %></span>
        </div>
        <div class="flex flex-col">
          <span class="text-slate-600 text-[8px] uppercase tracking-wider">rate/s</span>
          <span class={[
            "tabular-nums",
            cond do
              @stage.rate > 0 -> "text-emerald-300"
              true -> "text-slate-500"
            end
          ]}>
            <%= fmt_rate(@stage.rate) %>
          </span>
        </div>
        <div class="flex flex-col">
          <span class="text-slate-600 text-[8px] uppercase tracking-wider">queue</span>
          <span class="text-slate-300 tabular-nums"><%= compact(@stage.queue) %></span>
        </div>
        <div class="flex flex-col">
          <span class="text-slate-600 text-[8px] uppercase tracking-wider">total</span>
          <span class="text-slate-300 tabular-nums"><%= compact(@stage.total) %></span>
        </div>
      </div>
    </div>
    """
  end

  ## Formatting helpers (also re-exported for LiveViews via `import`)

  def fmt_rate(0), do: "0.0"
  def fmt_rate(0.0), do: "0.0"
  def fmt_rate(n) when is_number(n), do: :erlang.float_to_binary(n * 1.0, decimals: 1)
  def fmt_rate(_), do: "—"

  def compact(n) when is_integer(n) and n >= 1_000_000_000,
    do: "#{Float.round(n / 1_000_000_000, 2)}G"

  def compact(n) when is_integer(n) and n >= 1_000_000,
    do: "#{Float.round(n / 1_000_000, 2)}M"

  def compact(n) when is_integer(n) and n >= 1_000, do: "#{Float.round(n / 1_000, 1)}k"
  def compact(n) when is_integer(n), do: Integer.to_string(n)
  def compact(n) when is_float(n), do: :erlang.float_to_binary(n, decimals: 1)
  def compact(_), do: "—"

  def fmt_pct(n) when is_number(n), do: "#{:erlang.float_to_binary(n * 1.0, decimals: 1)}%"
  def fmt_pct(_), do: "—%"

  def fmt_ms_age(nil), do: "—"
  def fmt_ms_age(ms) when ms < 60_000, do: "#{div(ms, 1000)}s"
  def fmt_ms_age(ms) when ms < 3_600_000, do: "#{div(ms, 60_000)}m"
  def fmt_ms_age(ms) when ms < 86_400_000, do: "#{div(ms, 3_600_000)}h"
  def fmt_ms_age(ms), do: "#{div(ms, 86_400_000)}d"
end
