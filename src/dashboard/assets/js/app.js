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

const csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content")
const liveSocket = new LiveSocket("/live", Socket, {
  longPollFallbackMs: false,
  params: {_csrf_token: csrfToken},
  hooks: {...colocatedHooks, LiveViewWitness, WorkspacePipelineFlow},
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
