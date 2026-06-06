// If you want to use Phoenix channels, run `mix help phx.gen.channel`
// to get started and then uncomment the line below.
// import "./user_socket.js"

// You can include dependencies in two ways.
//
// The simplest option is to put them in assets/vendor and
// import them using relative paths:
//
//     import "../vendor/some-package.js"
//
// Alternatively, you can `npm install some-package --prefix assets` and import
// them using a path starting with the package name:
//
//     import "some-package"
//
// If you have dependencies that try to import CSS, esbuild will generate a separate `app.css` file.
// To load it, simply add a second `<link>` to your `root.html.heex` file.

// Include phoenix_html to handle method=PUT/DELETE in forms and buttons.
import "phoenix_html"
// Establish Phoenix Socket and LiveView configuration.
import {Socket} from "phoenix"
import {LiveSocket} from "phoenix_live_view"
import * as echarts from "echarts"
import {hooks as colocatedHooks} from "phoenix-colocated/axon_dashboard"
import topbar from "../vendor/topbar"
import LiveViewWitness from "../../../liveview_witness/priv/static/liveview_witness.js"

const WorkspacePipelineFlow = {
  mounted() {
    this.chart = echarts.init(this.el)
    this.handleEvent("workspace_pipeline_flow", (payload) => {
      this.renderChart(payload)
    })
    this.renderChart()
  },
  updated() {
    this.renderChart()
  },
  destroyed() {
    if (this.chart) this.chart.dispose()
  },
  renderChart(payload = null) {
    const known = Number((payload && payload.known) ?? this.el.dataset.known ?? 0)
    const completed = Number((payload && payload.completed) ?? this.el.dataset.completed ?? 0)
    const indexed = Number((payload && payload.completed_indexed) ?? this.el.dataset.indexed ?? 0)
    const indexedDegraded = Number((payload && payload.completed_indexed_degraded) ?? this.el.dataset.indexedDegraded ?? 0)
    const skipped = Number((payload && payload.completed_skipped) ?? this.el.dataset.skipped ?? 0)
    const deleted = Number((payload && payload.completed_deleted) ?? this.el.dataset.deleted ?? 0)
    const oversized = Number((payload && payload.completed_oversized) ?? this.el.dataset.oversized ?? 0)
    const indexing = Number((payload && payload.indexing) ?? this.el.dataset.indexing ?? 0)
    const pending = Number((payload && payload.pending) ?? this.el.dataset.pending ?? 0)
    const indexedGraphReady = Number((payload && payload.indexed_graph_ready) ?? this.el.dataset.indexedGraphReady ?? 0)
    const indexedGraphMissing = Number((payload && payload.indexed_graph_missing) ?? this.el.dataset.indexedGraphMissing ?? 0)
    const indexedDegradedGraphReady = Number((payload && payload.indexed_degraded_graph_ready) ?? this.el.dataset.indexedDegradedGraphReady ?? 0)
    const indexedDegradedGraphMissing = Number((payload && payload.indexed_degraded_graph_missing) ?? this.el.dataset.indexedDegradedGraphMissing ?? 0)
    const indexedVectorReady = Number((payload && payload.indexed_vector_ready) ?? this.el.dataset.indexedVectorReady ?? 0)
    const indexedVectorMissing = Number((payload && payload.indexed_vector_missing) ?? this.el.dataset.indexedVectorMissing ?? 0)
    const indexedDegradedVectorReady = Number((payload && payload.indexed_degraded_vector_ready) ?? this.el.dataset.indexedDegradedVectorReady ?? 0)
    const indexedDegradedVectorMissing = Number((payload && payload.indexed_degraded_vector_missing) ?? this.el.dataset.indexedDegradedVectorMissing ?? 0)

    const clamp = (value) => Math.max(0, Number.isFinite(value) ? value : 0)
    const activeIndexing = clamp(indexing)
    const activePending = clamp(pending)
    const edgeExplanations = {
      "Known Files|Indexing": "Files currently claimed and being processed.",
      "Known Files|Pending": "Files known by Axon but not yet started.",
      "Known Files|Indexed": "Files fully indexed with the primary path completed.",
      "Known Files|Indexed Degraded": "Files completed with degraded or partial indexing.",
      "Known Files|Skipped": "Files intentionally skipped by current policy.",
      "Known Files|Deleted": "Files removed from source and marked as deleted.",
      "Known Files|Oversized": "Files refused under the current budget envelope.",
      "Indexed|Indexed · AST Ready": "Indexed files whose AST-derived graph is available.",
      "Indexed|Indexed · AST Missing": "Indexed files still missing AST/graph truth.",
      "Indexed Degraded|Degraded · AST Ready": "Degraded indexed files whose AST-derived graph is available.",
      "Indexed Degraded|Degraded · AST Missing": "Degraded indexed files still missing AST/graph truth.",
      "Indexed · AST Ready|Indexed · Vectorized": "Indexed files whose vectorization is complete.",
      "Indexed · AST Ready|Indexed · Not Yet Vectorized": "Indexed files still awaiting vectorization.",
      "Degraded · AST Ready|Degraded · Vectorized": "Degraded indexed files whose vectorization is complete.",
      "Degraded · AST Ready|Degraded · Not Yet Vectorized": "Degraded indexed files still awaiting vectorization."
    }

    const link = (source, target, value, meta = {}) => ({
      source,
      target,
      value: clamp(value),
      description: edgeExplanations[`${source}|${target}`] ?? null,
      ...meta
    })
    const linksData = [
      link("Known Files", "Indexing", activeIndexing),
      link("Known Files", "Pending", activePending),
      link("Known Files", "Indexed", indexed),
      link("Known Files", "Indexed Degraded", indexedDegraded),
      link("Known Files", "Skipped", skipped),
      link("Known Files", "Deleted", deleted),
      link("Known Files", "Oversized", oversized),
      link("Indexed", "Indexed · AST Ready", indexedGraphReady),
      link("Indexed", "Indexed · AST Missing", indexedGraphMissing),
      link("Indexed Degraded", "Degraded · AST Ready", indexedDegradedGraphReady),
      link("Indexed Degraded", "Degraded · AST Missing", indexedDegradedGraphMissing),
      link("Indexed · AST Ready", "Indexed · Vectorized", indexedVectorReady),
      link("Indexed · AST Ready", "Indexed · Not Yet Vectorized", indexedVectorMissing),
      link("Degraded · AST Ready", "Degraded · Vectorized", indexedDegradedVectorReady),
      link("Degraded · AST Ready", "Degraded · Not Yet Vectorized", indexedDegradedVectorMissing)
    ].filter(item => item.value > 0)

    const nodePalette = {
      "Known Files": "#93c5fd",
      "Indexing": "#38bdf8",
      "Pending": "#fbbf24",
      "Indexed": "#10b981",
      "Indexed Degraded": "#fb923c",
      "Skipped": "#94a3b8",
      "Deleted": "#64748b",
      "Oversized": "#ef4444",
      "Indexed · AST Ready": "#22c55e",
      "Indexed · AST Missing": "#f97316",
      "Degraded · AST Ready": "#14b8a6",
      "Degraded · AST Missing": "#f97316",
      "Indexed · Vectorized": "#06b6d4",
      "Indexed · Not Yet Vectorized": "#a78bfa",
      "Degraded · Vectorized": "#0891b2",
      "Degraded · Not Yet Vectorized": "#c084fc"
    }

    const nodesData = [...new Set(linksData.flatMap(item => [item.source, item.target]))].map(name => ({
      name,
      itemStyle: {color: nodePalette[name] ?? "#94a3b8"}
    }))

    this.chart.setOption({
      animation: true,
      animationDuration: 450,
      animationDurationUpdate: 240,
      backgroundColor: "transparent",
      tooltip: {
        trigger: "item",
        formatter: (params) => {
          if (params.dataType === "edge") {
            const lines = [
              `${params.data.source} → ${params.data.target}`,
              `${params.data.value} files`
            ]
            if (params.data.description) lines.push(params.data.description)
            return lines.join("<br/>")
          }
          return `${params.name}`
        }
      },
      series: [
        {
          type: "sankey",
          data: nodesData,
          links: linksData,
          top: 24,
          bottom: 24,
          left: 12,
          right: 24,
          nodeAlign: "left",
          emphasis: {focus: "adjacency"},
          draggable: false,
          lineStyle: {
            color: "source",
            curveness: 0.45,
            opacity: 0.44
          },
          itemStyle: {
            borderWidth: 1,
            borderColor: "rgba(15, 23, 42, 0.55)"
          },
          label: {
            color: "#e2e8f0",
            fontSize: 11,
            fontWeight: 500
          }
        }
      ]
    }, true)
  }
}

// REQ-AXO-901647: pipeline V2 topology renderer.
// Renders the canonical CPT-AXO-054 flow:
//   A1 read+hash → A2 parse-TS → A3 graph-UPSERT
//     ─try_send (A3→B1 cap)──▶
//   B1 fetch → B2 embed-GPU → B3 write-emb
//
// Pure SVG (no echarts) to keep the topology static + crisp. The hook
// listens to a `pipeline_state` push_event from PipelineLive and updates
// the live values (worker counts, rate, buffer fill, GPU badge).
const PipelineTopology = {
  mounted() {
    this.el.innerHTML = this.buildSvgShell()
    this.handleEvent("pipeline_state", (state) => this.update(state))
  },
  destroyed() {
    this.el.innerHTML = ""
  },
  buildSvgShell() {
    const W = 1200
    const H = 320
    const stageY = 150
    const stageW = 130
    const stageH = 70
    const aXs = [60, 220, 380]
    const bXs = [700, 860, 1020]

    const stageBox = (x, id, label, group) => {
      const accent = group === "A" ? "#22d3ee" : "#34d399"
      const bg = group === "A" ? "rgba(34, 211, 238, 0.08)" : "rgba(52, 211, 153, 0.08)"
      return `
        <g id="stage-${id}" class="stage" data-group="${group}">
          <rect x="${x}" y="${stageY}" width="${stageW}" height="${stageH}" rx="10"
            fill="${bg}" stroke="${accent}" stroke-opacity="0.4" stroke-width="1.5"></rect>
          <text x="${x + stageW/2}" y="${stageY + 22}" text-anchor="middle"
            class="stage-name" fill="${accent}" font-family="ui-monospace, monospace"
            font-size="14" font-weight="700" letter-spacing="0.08em">${id.toUpperCase()}</text>
          <text x="${x + stageW/2}" y="${stageY + 40}" text-anchor="middle"
            class="stage-label" fill="#cbd5e1" font-family="ui-monospace, monospace"
            font-size="10">${label}</text>
          <text id="stage-${id}-workers" x="${x + stageW/2}" y="${stageY + 58}" text-anchor="middle"
            class="stage-workers" fill="#f8fafc" font-family="ui-monospace, monospace"
            font-size="11" font-weight="600">— workers</text>
          <circle cx="${x + stageW - 12}" cy="${stageY + 12}" r="3"
            id="stage-${id}-led" fill="${accent}" opacity="0.7">
            <animate attributeName="opacity" values="0.3;1;0.3" dur="1.8s" repeatCount="indefinite"/>
          </circle>
        </g>
      `
    }

    const arrow = (x1, x2, y) => `
      <line x1="${x1}" y1="${y}" x2="${x2 - 8}" y2="${y}" stroke="rgba(148,163,184,0.45)" stroke-width="1.5"/>
      <polygon points="${x2 - 8},${y - 4} ${x2 - 2},${y} ${x2 - 8},${y + 4}" fill="rgba(148,163,184,0.45)"/>
    `

    // A3 → B hand-off (slice 4/5 SOTA): A3 writes chunks to PG; the
    // trg_chunk_notify_pending trigger fires pg_notify('chunk_pending_embed');
    // demand_pull_b LISTENs + adaptively polls and feeds B2 directly via the
    // b_chunks channel. There is NO try_send and NO B1 stage — the A3→B1 push
    // channel was eliminated (REQ-AXO-901746); B is fed exclusively by demand_pull.
    const bufX1 = aXs[2] + stageW + 10
    const bufX2 = bXs[1] - 10
    const bufW = bufX2 - bufX1
    const bufY = stageY + stageH/2 - 14
    const bufH = 28

    const groupHeader = (x, label, color) => `
      <text x="${x}" y="100" fill="${color}" font-family="ui-monospace, monospace"
        font-size="11" font-weight="600" letter-spacing="0.2em">${label}</text>
    `

    return `
    <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid meet" width="100%" height="100%"
        style="background: radial-gradient(circle at 20% 30%, rgba(34,211,238,0.04), transparent 40%), radial-gradient(circle at 80% 70%, rgba(52,211,153,0.04), transparent 40%);">
      <defs>
        <linearGradient id="bufGrad" x1="0" x2="1">
          <stop offset="0%" stop-color="#fbbf24" stop-opacity="0.6"/>
          <stop offset="100%" stop-color="#10b981" stop-opacity="0.6"/>
        </linearGradient>
      </defs>

      ${groupHeader(60, "PIPELINE A · CPU (graph + chunks + FTS)", "#22d3ee")}
      ${groupHeader(700, "PIPELINE B · GPU embedding", "#34d399")}

      ${stageBox(aXs[0], "a1", "read + hash", "A")}
      ${stageBox(aXs[1], "a2", "parse TS", "A")}
      ${stageBox(aXs[2], "a3", "graph UPSERT", "A")}
      ${stageBox(bXs[1], "b2", "embed GPU", "B")}
      ${stageBox(bXs[2], "b3", "write embeddings", "B")}

      ${arrow(aXs[0] + stageW, aXs[1], stageY + stageH/2)}
      ${arrow(aXs[1] + stageW, aXs[2], stageY + stageH/2)}
      ${arrow(bufX2, bXs[1], stageY + stageH/2)}
      ${arrow(bXs[1] + stageW, bXs[2], stageY + stageH/2)}

      <g id="buffer">
        <rect x="${bufX1}" y="${bufY}" width="${bufW}" height="${bufH}" rx="6"
          fill="rgba(15,23,42,0.6)" stroke="rgba(148,163,184,0.3)" stroke-dasharray="4 3"></rect>
        <rect x="${bufX1 + 2}" y="${bufY + 2}" width="0" height="${bufH - 4}" rx="4"
          id="buffer-fill" fill="url(#bufGrad)"></rect>
        <text x="${bufX1 + bufW/2}" y="${bufY - 6}" text-anchor="middle"
          fill="#94a3b8" font-family="ui-monospace, monospace" font-size="9" letter-spacing="0.15em">
          pg_notify → demand_pull_b → B2
        </text>
        <text id="buffer-label" x="${bufX1 + bufW/2}" y="${bufY + bufH + 14}" text-anchor="middle"
          fill="#cbd5e1" font-family="ui-monospace, monospace" font-size="10">b_chunks → B2</text>
      </g>

      <g id="rate-badge" transform="translate(${bXs[1] + stageW/2 - 50}, ${stageY + stageH + 24})">
        <rect x="0" y="0" width="100" height="32" rx="6" fill="rgba(15,23,42,0.7)" stroke="rgba(52,211,153,0.4)"/>
        <text id="rate-value" x="50" y="14" text-anchor="middle" fill="#34d399"
          font-family="ui-monospace, monospace" font-size="14" font-weight="700">—</text>
        <text x="50" y="27" text-anchor="middle" fill="#94a3b8"
          font-family="ui-monospace, monospace" font-size="8" letter-spacing="0.18em">CHUNKS/SEC</text>
      </g>

      <g id="gpu-badge" transform="translate(${bXs[1] + stageW/2 - 50}, ${stageY - 50})">
        <rect x="0" y="0" width="100" height="26" rx="6" id="gpu-rect"
          fill="rgba(245,158,11,0.15)" stroke="rgba(245,158,11,0.5)"/>
        <text x="50" y="12" text-anchor="middle" fill="#94a3b8"
          font-family="ui-monospace, monospace" font-size="8" letter-spacing="0.2em">PROVIDER</text>
        <text id="gpu-label" x="50" y="22" text-anchor="middle" fill="#f59e0b"
          font-family="ui-monospace, monospace" font-size="11" font-weight="700">—</text>
      </g>
    </svg>`
  },
  update(state) {
    if (!state || !state.stages) return
    state.stages.forEach((s) => {
      const el = this.el.querySelector(`#stage-${s.id}-workers`)
      if (el) el.textContent = `${s.workers} workers`
    })
    // b_chunks channel (demand_pull_b → B2). All values are canonical: cap +
    // fill come from the pushed payload (sourced from runtime_config / live
    // telemetry) — NO hardcoded literal. Absent ⇒ render "—", never a guess.
    const buf = state.buffer || {}
    const bCap = buf.cap || 0
    const bFill = buf.fill || 0
    const bufLabel = this.el.querySelector("#buffer-label")
    const bufFill = this.el.querySelector("#buffer-fill")
    if (bufLabel) bufLabel.textContent = bCap > 0
      ? `b_chunks ${bFill.toLocaleString()}/${bCap.toLocaleString()}`
      : "b_chunks —"
    if (bufFill && bCap > 0) {
      const w = Math.max(0, Math.min(1, bFill / bCap))
      const bufW = 78
      bufFill.setAttribute("width", String(bufW * w))
    }
    // Rate
    const rate = this.el.querySelector("#rate-value")
    if (rate) rate.textContent = (state.rate || 0).toFixed(1)
    // GPU
    const gpuLabel = this.el.querySelector("#gpu-label")
    const gpuRect = this.el.querySelector("#gpu-rect")
    if (gpuLabel) gpuLabel.textContent = (state.gpu || "?").toUpperCase()
    if (gpuRect) {
      const isOk = state.gpu === "cuda" || state.gpu === "tensorrt"
      gpuRect.setAttribute("fill", isOk ? "rgba(52,211,153,0.15)" : "rgba(245,158,11,0.15)")
      gpuRect.setAttribute("stroke", isOk ? "rgba(52,211,153,0.5)" : "rgba(245,158,11,0.5)")
      if (gpuLabel) gpuLabel.setAttribute("fill", isOk ? "#34d399" : "#f59e0b")
    }
  }
}

const csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content")
const liveSocket = new LiveSocket("/live", Socket, {
  longPollFallbackMs: false,
  params: {_csrf_token: csrfToken},
  hooks: {...colocatedHooks, LiveViewWitness, WorkspacePipelineFlow, PipelineTopology},
})

// Show progress bar on live navigation and form submits
topbar.config({barColors: {0: "#29d"}, shadowColor: "rgba(0, 0, 0, .3)"})
window.addEventListener("phx:page-loading-start", _info => topbar.show(300))
window.addEventListener("phx:page-loading-stop", _info => topbar.hide())

// connect if there are any LiveViews on the page
liveSocket.connect()

// expose liveSocket on window for web console debug logs and latency simulation:
// >> liveSocket.enableDebug()
// >> liveSocket.enableLatencySim(1000)  // enabled for duration of browser session
// >> liveSocket.disableLatencySim()
window.liveSocket = liveSocket

// The lines below enable quality of life phoenix_live_reload
// development features:
//
//     1. stream server logs to the browser console
//     2. click on elements to jump to their definitions in your code editor
//
if (process.env.NODE_ENV === "development") {
  window.addEventListener("phx:live_reload:attached", ({detail: reloader}) => {
    // Enable server log streaming to client.
    // Disable with reloader.disableServerLogs()
    reloader.enableServerLogs()

    // Open configured PLUG_EDITOR at file:line of the clicked element's HEEx component
    //
    //   * click with "c" key pressed to open at caller location
    //   * click with "d" key pressed to open at function component definition location
    let keyDown
    window.addEventListener("keydown", e => keyDown = e.key)
    window.addEventListener("keyup", _e => keyDown = null)
    window.addEventListener("click", e => {
      if(keyDown === "c"){
        e.preventDefault()
        e.stopImmediatePropagation()
        reloader.openEditorAtCaller(e.target)
      } else if(keyDown === "d"){
        e.preventDefault()
        e.stopImmediatePropagation()
        reloader.openEditorAtDef(e.target)
      }
    }, true)

    window.liveReloader = reloader
  })
}
