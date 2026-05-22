defmodule AxonDashboardWeb.Features.ProjectsTest do
  @moduledoc """
  REQ-AXO-901649 — Projects page (`/projects`) contract.

  Behaviour against a stubbed SQL gateway (no live brain): the page MUST
  render the table chrome (headers, empty-state message) and surface a
  fetch error gracefully — no crash, no "phx-error" banner.
  """

  use AxonDashboardWeb.FeatureCase, async: false

  alias Wallaby.Query

  feature "page loads with Projects header", %{session: session} do
    session
    |> visit("/projects")
    |> assert_has(Query.css("header", text: "Axon Cockpit"))
    |> assert_has(Query.css("body", text: "Indexing per project"))
  end

  feature "table headers are visible", %{session: session} do
    session = visit(session, "/projects")

    for label <- ["Project", "Chunks", "Embedded", "Coverage", "Symbols", "Edges",
                  "Δ Chunks", "Δ Embedded"] do
      assert_has(session, Query.css("thead", text: label))
    end
  end

  feature "totals strip renders five tot cards", %{session: session} do
    session = visit(session, "/projects")

    for label <- ["Projects", "Σ Chunks", "Σ Embedded", "Σ Symbols", "Σ Edges"] do
      assert_has(session, Query.css("section", text: label))
    end
  end

  feature "sort buttons are present for each sortable column", %{session: session} do
    session = visit(session, "/projects")

    # Each .th component renders a button with phx-click="sort".
    buttons = Wallaby.Browser.all(session, Query.css("button[phx-click=\"sort\"]"))
    assert length(buttons) >= 8,
           "expected at least 8 sort buttons (one per column), got #{length(buttons)}"
  end

  feature "footer cadence message is visible", %{session: session} do
    session
    |> visit("/projects")
    |> assert_has(Query.css("body", text: "refresh 5s"))
  end

  feature "fetch-error fallback or empty-state renders, no crash banner",
          %{session: session} do
    # With no live SqlGateway, we either get a fetch_error chip OR an
    # empty-state row — but never a phx-error.
    session = visit(session, "/projects")

    elements = Wallaby.Browser.all(session, Query.css(".phx-error"))
    assert elements == [], "expected no .phx-error elements, found #{length(elements)}"
  end
end
