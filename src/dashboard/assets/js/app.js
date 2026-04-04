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
import {hooks as colocatedHooks} from "phoenix-colocated/axon_dashboard"
import * as echarts from "echarts"
import topbar from "../vendor/topbar"
import LiveViewWitness from "../../../liveview_witness/priv/static/liveview_witness.js"

const WorkspaceSunburst = {
  mounted() {
    this.chart = echarts.init(this.el)
    this._onResize = () => this.chart && this.chart.resize()
    window.addEventListener("resize", this._onResize)
    this.renderSunburst()
  },
  updated() {
    this.renderSunburst()
  },
  destroyed() {
    window.removeEventListener("resize", this._onResize)
    if (this.chart) {
      this.chart.dispose()
      this.chart = null
    }
  },
  renderSunburst() {
    if (!this.chart) return

    const read = key => {
      const value = Number(this.el.dataset[key] || 0)
      return Number.isFinite(value) ? Math.max(0, Math.round(value)) : 0
    }

    const known = read("known")
    const indexed = read("indexed")
    const degraded = read("indexedDegraded")
    const pending = read("pending")
    const indexing = read("indexing")
    const oversized = read("oversized")
    const skipped = read("skipped")
    const deleted = read("deleted")
    const graphReady = read("graphReady")
    const vectorReadyFile = read("vectorReadyFile")

    const completed = indexed + degraded + skipped + deleted
    const vectorInIndexed = Math.min(indexed, vectorReadyFile)
    const graphInIndexed = Math.min(indexed, graphReady)
    const graphOnlyInIndexed = Math.max(0, graphInIndexed - vectorInIndexed)
    const notReadyInIndexed = Math.max(0, indexed - vectorInIndexed - graphOnlyInIndexed)

    const data = [
      {
        name: "Completed",
        value: completed,
        children: [
          {
            name: "Indexed",
            value: indexed,
            children: [
              {name: "Vector Ready", value: vectorInIndexed},
              {name: "Graph Only", value: graphOnlyInIndexed},
              {name: "Not Ready", value: notReadyInIndexed},
            ],
          },
          {name: "Degraded", value: degraded},
          {name: "Skipped", value: skipped},
          {name: "Deleted", value: deleted},
        ],
      },
      {name: "Pending", value: pending},
      {name: "Indexing", value: indexing},
      {name: "Oversized", value: oversized},
    ]

    this.chart.setOption(
      {
        animationDuration: 300,
        backgroundColor: "transparent",
        tooltip: {
          trigger: "item",
          formatter: params => `${params.name}: ${params.value ?? 0}`,
        },
        series: [
          {
            type: "sunburst",
            radius: ["18%", "92%"],
            sort: null,
            nodeClick: false,
            data: [{name: "Known Files", value: known, children: data}],
            levels: [
              {},
              {r0: "18%", r: "45%", itemStyle: {borderWidth: 2, borderColor: "#0f172a"}},
              {r0: "45%", r: "70%", itemStyle: {borderWidth: 2, borderColor: "#0f172a"}},
              {r0: "70%", r: "92%", itemStyle: {borderWidth: 2, borderColor: "#0f172a"}},
            ],
            label: {
              color: "#e5e7eb",
              rotate: "radial",
              fontSize: 11,
            },
            itemStyle: {
              borderRadius: 6,
            },
            color: [
              "#22c55e",
              "#3b82f6",
              "#f59e0b",
              "#ef4444",
              "#a78bfa",
              "#06b6d4",
              "#84cc16",
              "#f97316",
            ],
          },
        ],
      },
      true
    )
  },
}

const csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content")
const liveSocket = new LiveSocket("/live", Socket, {
  longPollFallbackMs: 2500,
  params: {_csrf_token: csrfToken},
  hooks: {...colocatedHooks, LiveViewWitness, WorkspaceSunburst},
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
