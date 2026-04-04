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

const WorkspaceSunburst = {
  mounted() {
    this.chart = echarts.init(this.el)
    this.handleEvent("workspace_sunburst", (payload) => {
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
    const graphReady = Number((payload && payload.graph_ready) ?? this.el.dataset.graphReady ?? 0)
    const vectorFile = Number((payload && payload.vector_ready_file) ?? this.el.dataset.vectorFile ?? 0)
    const vectorGraph = Number((payload && payload.vector_ready_graph) ?? this.el.dataset.vectorGraph ?? 0)

    const total = Math.max(known, 1)
    const clamp = (value, max) => Math.max(0, Math.min(value, max))

    const cCompleted = clamp(completed, total)
    const cGraph = clamp(graphReady, cCompleted)
    const cVectorFile = clamp(vectorFile, cGraph)
    const cVectorGraph = clamp(vectorGraph, cVectorFile)

    const data = [{
      name: "Known Files",
      value: total,
      children: [
        {
          name: "Completed",
          value: cCompleted,
          children: [
            {
              name: "Graph Ready",
              value: cGraph,
              children: [
                {
                  name: "Vector File Ready",
                  value: cVectorFile,
                  children: [
                    {name: "Vector Graph Ready", value: cVectorGraph},
                    {name: "Vector Graph Pending", value: cVectorFile - cVectorGraph}
                  ]
                },
                {name: "Vector File Pending", value: cGraph - cVectorFile}
              ]
            },
            {name: "Graph Pending", value: cCompleted - cGraph}
          ]
        },
        {name: "Not Completed", value: total - cCompleted}
      ]
    }]

    this.chart.setOption({
      animation: true,
      animationDuration: 350,
      animationDurationUpdate: 220,
      backgroundColor: "transparent",
      tooltip: {trigger: "item"},
      color: ["#5470C6", "#91CC75", "#FAC858", "#EE6666", "#73C0DE", "#3BA272", "#FC8452", "#9A60B4", "#EA7CCC"],
      series: [
        {
          type: "sunburst",
          data,
          radius: ["10%", "92%"],
          sort: null,
          emphasis: {focus: "ancestor"},
          itemStyle: {borderWidth: 2, borderColor: "#0b1220"},
          label: {rotate: "radial", color: "#f8fafc", fontSize: 11, overflow: "truncate"},
          levels: [
            {},
            {r0: "10%", r: "26%", itemStyle: {borderWidth: 2}, label: {fontSize: 12}},
            {r0: "26%", r: "44%", itemStyle: {borderWidth: 2}},
            {r0: "44%", r: "66%", itemStyle: {borderWidth: 2}},
            {r0: "66%", r: "92%", itemStyle: {borderWidth: 2}, label: {fontSize: 10}}
          ]
        }
      ]
    }, true)
  }
}

const csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content")
const liveSocket = new LiveSocket("/live", Socket, {
  longPollFallbackMs: false,
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
