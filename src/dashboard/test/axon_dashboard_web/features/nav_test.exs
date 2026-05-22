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
    session = visit(session, "/")

    # Pipeline → Projects
    session
    |> click(Query.link("Projects"))
    |> assert_has(Query.css("[href=\"/projects\"][aria-current=page], a[href=\"/projects\"]"))

    # we expect URL to be /projects now
    assert current_path(session) == "/projects"

    # Projects → MCP
    session = click(session, Query.link("MCP"))
    assert current_path(session) == "/mcp"

    # MCP → Pipeline (root)
    session = click(session, Query.link("Pipeline"))
    assert current_path(session) == "/"
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
    for {path, _current, _label} <- @routes do
      session
      |> visit(path)
      |> assert_has(Query.css("header", text: "Axon Cockpit"))
      |> assert_has(Query.css("header", text: "Structural Intelligence"))
      |> assert_has(Query.css("footer", text: "install"))
      |> assert_has(Query.css("footer", text: "heartbeat age"))
    end
  end

  feature "mode / instance / build chips render on every route", %{session: session} do
    # The chips use lowercase labels ("mode" / "instance" / "build")
    # rendered with uppercase via Tailwind CSS — assertion is text-based
    # so it survives CSS changes.
    for {path, _current, _label} <- @routes do
      session = visit(session, path)

      session
      |> assert_has(Query.css("header", text: "mode"))
      |> assert_has(Query.css("header", text: "instance"))
      |> assert_has(Query.css("header", text: "build"))
    end
  end
end
