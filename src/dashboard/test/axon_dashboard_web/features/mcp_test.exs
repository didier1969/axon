defmodule AxonDashboardWeb.Features.McpTest do
  @moduledoc """
  REQ-AXO-901649 — MCP catalog (`/mcp`) contract.

  Tools come from `test/support/fixtures/mcp_tools.json` via the
  `AXON_MCP_FIXTURE_PATH` env var injected in `test_helper.exs`. This keeps
  the suite hermetic — no live brain dependency — while still exercising
  the same `handle_event("filter", …)` and `handle_event("category", …)`
  paths that production traffic hits.

  This module also pins the empty-filter regression fix: with 0 tools and
  filter == "" the page must NOT say "No tools match filter "".
  """

  use AxonDashboardWeb.FeatureCase, async: false

  alias Wallaby.Query

  feature "page loads with 68 public tools header", %{session: session} do
    session
    |> visit("/mcp")
    |> assert_has(Query.css("h1", text: "68 public tools"))
  end

  feature "all six category tabs are visible", %{session: session} do
    session = visit(session, "/mcp")

    # The :all tab is labelled "All (68)"; the others have category-specific
    # counts (DX/SOLL/Graph/System/Other).
    for label <- ["All", "DX", "SOLL", "Graph", "System", "Other"] do
      assert_has(session, Query.css("button[phx-click=\"category\"]", text: label))
    end
  end

  feature "clicking DX tab restricts the visible tools to the DX category",
          %{session: session} do
    session
    |> visit("/mcp")
    |> click(Query.css("button[phx-value-value=\"dx\"]"))

    # Should show DX category header (cat_label/1 = "DX · structural intelligence")
    assert_has(session, Query.css("section h2", text: "DX"))

    # And NOT show SOLL-only tools like soll_manager
    refute_has(session, Query.css("code", text: "soll_manager"))
  end

  feature "clicking ALL tab restores every tool", %{session: session} do
    session =
      session
      |> visit("/mcp")
      |> click(Query.css("button[phx-value-value=\"dx\"]"))
      |> click(Query.css("button[phx-value-value=\"all\"]"))

    # Both a DX-tool and a SOLL-tool should be present.
    assert_has(session, Query.css("code", text: "query"))
    assert_has(session, Query.css("code", text: "soll_manager"))
  end

  feature "typing a filter narrows the list to matching tools", %{session: session} do
    session =
      session
      |> visit("/mcp")
      |> fill_in(Query.css("input[phx-keyup=\"filter\"]"), with: "soll")

    # soll_manager must remain visible.
    assert_has(session, Query.css("code", text: "soll_manager"))

    # A pure DX tool like "embedding_status" should disappear.
    refute_has(session, Query.css("code", text: "embedding_status"))
  end

  feature "non-matching filter shows the No-tools-match message",
          %{session: session} do
    session =
      session
      |> visit("/mcp")
      |> fill_in(Query.css("input[phx-keyup=\"filter\"]"), with: "xyzdoesnotexist")

    assert_has(session, Query.css("body", text: "No tools match filter"))
    assert_has(session, Query.css("body", text: "xyzdoesnotexist"))
  end

  feature "clearing the filter restores every tool, no no-match banner",
          %{session: session} do
    session =
      session
      |> visit("/mcp")
      |> fill_in(Query.css("input[phx-keyup=\"filter\"]"), with: "xyzdoesnotexist")
      # Now clear it
      |> clear(Query.css("input[phx-keyup=\"filter\"]"))

    refute_has(session, Query.css("body", text: "No tools match filter"))
    assert_has(session, Query.css("code", text: "soll_manager"))
    assert_has(session, Query.css("code", text: "query"))
  end

  feature "fast typing then full backspace ends in clean state (REQ-AXO-901649 race-free)",
          %{session: session} do
    field = Query.css("input[phx-keyup=\"filter\"]")

    session =
      session
      |> visit("/mcp")
      |> fill_in(field, with: "abcdefghij")
      |> clear(field)

    refute_has(session, Query.css("body", text: "No tools match filter"))
    assert_has(session, Query.css("code", text: "soll_manager"))
  end
end
