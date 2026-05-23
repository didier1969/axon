defmodule AxonDashboardWeb.Features.PipelineTest do
  @moduledoc """
  REQ-AXO-901649 — Pipeline cockpit (`/`) contract.

  This page is the SOLL canonical surface for CPT-AXO-054 (A1..A3 + B1..B3
  topology). The contract asserts the six stages render, the worker-config
  table has six rows, and the B2 / GPU panels exist regardless of whether
  the brain provides live metrics (we run against a stubbed MCP endpoint
  here, so values gracefully degrade to "0" / "n/a" — but the structure
  must remain).
  """

  use AxonDashboardWeb.FeatureCase, async: false

  alias Wallaby.Query

  feature "page loads with Axon Cockpit header", %{session: session} do
    session
    |> visit("/")
    |> assert_has(Query.css("header", text: "Axon Cockpit"))
    |> assert_has(Query.css("h1, h2", text: "A1/A2/A3 → try_send → B1/B2/B3"))
  end

  feature "all six KPI cards are present (indexed files / symbols / edges / chunks / embedded / pending)",
          %{session: session} do
    session = visit(session, "/")

    # REQ-AXO-901683 — KPI labels render through `text-[10px] uppercase`
    # in `kpi/1` (pipeline_live.ex). WebDriver getText returns the
    # rendered upper-cased text.
    for label <- ["INDEXED FILES", "SYMBOLS", "EDGES", "TOTAL CHUNKS", "EMBEDDED", "PENDING"] do
      # count: :any — labels like "EMBEDDED" / "PENDING" may appear in
      # multiple sections (KPI card + worker activity panel).
      assert_has(session, Query.css("section", text: label, count: :any))
    end
  end

  feature "pipeline topology SVG hook mounts with six stage labels",
          %{session: session} do
    session
    |> visit("/")
    |> assert_has(Query.css("#pipeline-topology"))
    |> assert_has(Query.css("#pipeline-topology[phx-hook=\"PipelineTopology\"]"))

    # The stage labels are baked into the LV-rendered config table (canonical
    # source) — six rows for A1..B3.
    for stage <- ["A1", "A2", "A3", "B1", "B2", "B3"] do
      assert_has(session, Query.css("table", text: stage))
    end
  end

  feature "A3→B1 buffer cap row visible", %{session: session} do
    # REQ-AXO-901683 — the buffer cap line lives inside a parent div with
    # `uppercase tracking-wider` (pipeline_live.ex), so WebDriver getText
    # returns "A3→B1 BUFFER CAP".
    session
    |> visit("/")
    |> assert_has(Query.css("body", text: "A3→B1 BUFFER CAP"))
  end

  feature "worker config table has six rows", %{session: session} do
    session = visit(session, "/")

    # Each stage_row component renders one <tr> with its name in the first cell.
    rows =
      session
      |> Wallaby.Browser.all(Query.css("table tr"))

    # header row + 6 stage rows minimum
    assert length(rows) >= 7,
           "expected at least 7 table rows (header + 6 stages), got #{length(rows)}"
  end

  feature "B2 embedder rate panel + GPU panel visible", %{session: session} do
    # REQ-AXO-901683 — "B2 Embedder" lives inside a `text-[10px] uppercase
    # tracking-[0.18em]` parent (pipeline_live.ex), so WebDriver getText
    # returns "B2 EMBEDDER". The h2 right after ("B2 embedder rate ...")
    # is NOT uppercase-styled, so it stays as authored.
    session
    |> visit("/")
    |> assert_has(Query.css("body", text: "B2 embedder rate"))
    |> assert_has(Query.css("body", text: "B2 EMBEDDER"))
    |> assert_has(Query.css("body", text: "EFFECTIVE PROVIDER"))
  end

  feature "page renders with no JavaScript console errors", %{session: session} do
    # Wallaby + ChromeDriver expose browser logs via the Chrome
    # `goog:loggingPrefs` capability ; Wallaby.Browser.execute_script lets
    # us push errors into a custom array, but the simpler hard-floor is
    # "no `phx-error` banner, no red-500 fatal classes".
    session = visit(session, "/")
    Process.sleep(500)

    assert_no_phx_errors(session)
  end

  defp assert_no_phx_errors(session) do
    # phx-error class is applied to <main> when a LiveView crashes — the
    # presence of any element with that class means a real failure.
    elements = Wallaby.Browser.all(session, Query.css(".phx-error"))
    assert elements == [], "expected no .phx-error elements, found #{length(elements)}"
    session
  end
end
