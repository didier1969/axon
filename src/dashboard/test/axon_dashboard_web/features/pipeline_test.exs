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
    # REQ-AXO-902572 puis REQ-AXO-902575 — deux corrections successives, et
    # cette fois c'est le TEST qui avait raison de suivre la page, mais la PAGE
    # qui décrivait un mécanisme mort. B1/`try_send` (session 19) puis
    # `PG NOTIFY`/`demand_pull_b` (étape intermédiaire) sont TOUS retirés
    # (REQ-AXO-901975 / DEC-AXO-901631). Le feeder réel est le drain trié.
    |> assert_has(
      Query.css("h1, h2", text: "A1/A2/A3 → embed_status pending → drain trié → B2/B3")
    )
  end

  feature "all six KPI cards are present (indexed files / symbols / edges / chunks / embedded / pending)",
          %{session: session} do
    session = visit(session, "/")

    # REQ-AXO-901683 — KPI labels render through `text-[10px] uppercase`
    # in `kpi/1` (pipeline_live.ex). WebDriver getText returns the
    # rendered upper-cased text.
    # REQ-AXO-902570 — l'INTERFACE avait raison : trois libellés ont été
    # renommés dans `kpi/1` (pipeline_live.ex:119-168). "INDEXED FILES" →
    # "Indexed", "TOTAL CHUNKS" → "Chunks", "EMBEDDED" → "Embeddings".
    #
    # Le match doit être EXACT, sur le libellé du composant `kpi/1` lui-même
    # (`tracking-[0.14em]`) et non sur la `section` entière. Un `text:` de
    # Wallaby cherche une sous-chaîne : sur un conteneur de la taille d'une
    # section, "CHUNKS" serait satisfait par la carte "Ready Chunks" et
    # "SYMBOLS" par "No symbols" — l'assertion passerait au vert sans jamais
    # exercer la carte qu'elle prétend vérifier.
    rendered =
      session
      |> Wallaby.Browser.all(Query.css(~s|div[class*="tracking-[0.14em]"]|))
      |> Enum.map(&(&1 |> Wallaby.Element.text() |> String.trim()))

    for label <- ["INDEXED", "SYMBOLS", "EDGES", "CHUNKS", "EMBEDDINGS", "PENDING"] do
      assert label in rendered,
             "carte KPI #{label} absente — libellés rendus : #{inspect(rendered)}"
    end
  end

  feature "pipeline topology SVG hook mounts with five stage labels",
          %{session: session} do
    session
    |> visit("/")
    |> assert_has(Query.css("#pipeline-topology"))
    |> assert_has(Query.css("#pipeline-topology[phx-hook=\"PipelineTopology\"]"))

    # The stage labels are baked into the LV-rendered config table (canonical
    # source). REQ-AXO-902572 — l'INTERFACE avait raison : cinq étages, pas six.
    # B1 n'est plus un pool de workers (`PipelineBWorkerCounts` ne porte que
    # `b2`/`b3`) ; `AXON_B1_WORKERS` est mort avec lui.
    for stage <- ["A1", "A2", "A3", "B2", "B3"] do
      assert_has(session, Query.css("table", text: stage))
    end
  end

  feature "b_chunks channel cap row visible", %{session: session} do
    # REQ-AXO-901683 — the cap line lives inside a parent div with
    # `uppercase tracking-wider` (pipeline_live.ex:202), so WebDriver getText
    # upper-cases it.
    # REQ-AXO-902572 — l'INTERFACE avait raison : il n'y a plus de tampon
    # A3→B1. Le canal interne restant est `b_chunks`, en `send().await` avec
    # vraie contre-pression, et non plus un `try_send` à rejet silencieux.
    session
    |> visit("/")
    |> assert_has(Query.css("body", text: "B_CHUNKS CAP"))
  end

  feature "worker config table has five stage rows", %{session: session} do
    session = visit(session, "/")

    # Each stage_row component renders one <tr> with its name in the first cell.
    rows =
      session
      |> Wallaby.Browser.all(Query.css("table tr"))

    # REQ-AXO-902572 — l'INTERFACE avait raison : en-tête + CINQ étages
    # (A1, A2, A3, B2, B3). B1 a été absorbé dans `demand_pull_b`, il n'a plus
    # de ligne de configuration parce qu'il n'a plus de workers à configurer.
    assert length(rows) >= 6,
           "expected at least 6 table rows (header + 5 stages), got #{length(rows)}"
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
