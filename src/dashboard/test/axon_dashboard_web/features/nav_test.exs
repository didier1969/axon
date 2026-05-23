defmodule AxonDashboardWeb.Features.NavTest do
  @moduledoc """
  REQ-AXO-901649 — top-level navigation chrome contract.

  Asserts the same chrome (logo, nav links, mode/instance/build chips,
  install footer) renders on every route, and that clicking each link
  reaches the matching LiveView with the active highlight applied.
  """

  use AxonDashboardWeb.FeatureCase, async: false

  alias Wallaby.Query

  @routes [
    {"/", :pipeline, "Pipeline"},
    {"/projects", :projects, "Projects"},
    {"/mcp", :mcp, "MCP"}
  ]

  feature "all three routes return 200 and render the cockpit shell", %{session: session} do
    for {path, _current, _label} <- @routes do
      session
      |> visit(path)
      |> assert_has(Query.css("header", text: "Axon Cockpit"))
      |> assert_has(Query.css("footer", text: "REQ-AXO-901647"))
    end
  end

  feature "top-nav links route to /, /projects, /mcp", %{session: session} do
    session = session |> visit("/") |> wait_for_live()

    # Pipeline → Projects. REQ-AXO-901683 — Phoenix `.link navigate={…}`
    # renders an `<a data-phx-link>` that the LV JS hooks into for
    # `live_redirect`. The navigation runs through the WebSocket, so we
    # need both `wait_for_live` BEFORE clicking and an assertion that
    # the destination DOM rendered AFTER clicking — `current_path/1`
    # alone races with the network round-trip.
    session = click(session, Query.link("Projects"))
    # Wait until the LV pushState lands the new URL (live_redirect is
    # async over the WebSocket).
    wait_until_path(session, "/projects")
    assert current_path(session) == "/projects"

    # Projects → MCP
    session = session |> wait_for_live() |> click(Query.link("MCP"))
    wait_until_path(session, "/mcp")
    assert current_path(session) == "/mcp"

    # MCP → Pipeline (root)
    session = session |> wait_for_live() |> click(Query.link("Pipeline"))
    wait_until_path(session, "/")
    assert current_path(session) == "/"
  end

  defp wait_until_path(session, expected, attempts \\ 50) do
    cond do
      current_path(session) == expected ->
        :ok

      attempts <= 0 ->
        :timeout

      true ->
        Process.sleep(100)
        wait_until_path(session, expected, attempts - 1)
    end
  end

  feature "active nav link is highlighted on each route", %{session: session} do
    # When on /, the Pipeline link has amber-500/30 border (current?).
    session
    |> visit("/")
    |> assert_has(Query.css("a[href=\"/\"].border-amber-500\\/30", count: 1))

    session
    |> visit("/projects")
    |> assert_has(Query.css("a[href=\"/projects\"].border-amber-500\\/30", count: 1))

    session
    |> visit("/mcp")
    |> assert_has(Query.css("a[href=\"/mcp\"].border-amber-500\\/30", count: 1))
  end

  feature "logo and shell footer visible on every route", %{session: session} do
    # REQ-AXO-901683 — "Structural Intelligence", "install" and
    # "heartbeat age" are wrapped in `text-transform: uppercase` Tailwind
    # classes ; WebDriver's `getText` returns the visual text — see
    # CSS in lib/axon_dashboard_web/live/nav.ex (chip label + footer).
    for {path, _current, _label} <- @routes do
      session
      |> visit(path)
      |> assert_has(Query.css("header", text: "Axon Cockpit", count: :any))
      |> assert_has(Query.css("header", text: "STRUCTURAL INTELLIGENCE", count: :any))
      |> assert_has(Query.css("footer", text: "INSTALL", count: :any))
      |> assert_has(Query.css("footer", text: "HEARTBEAT AGE", count: :any))
    end
  end

  feature "mode / instance / build chips render on every route", %{session: session} do
    # REQ-AXO-901683 — chip labels are wrapped in `<span class="uppercase">`
    # (see `chip/1` in lib/axon_dashboard_web/live/nav.ex). WebDriver's
    # `getText` returns the rendered upper-cased text, so assert against
    # "MODE" / "INSTANCE" / "BUILD" — not their DOM source casing.
    for {path, _current, _label} <- @routes do
      session = visit(session, path)

      session
      |> assert_has(Query.css("header", text: "MODE", count: :any))
      |> assert_has(Query.css("header", text: "INSTANCE", count: :any))
      |> assert_has(Query.css("header", text: "BUILD", count: :any))
    end
  end
end
