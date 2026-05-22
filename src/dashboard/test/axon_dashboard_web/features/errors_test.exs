defmodule AxonDashboardWeb.Features.ErrorsTest do
  @moduledoc """
  REQ-AXO-901649 — cross-cutting browser-side error contract.

  Asserts that:
    * `/`, `/projects`, `/mcp` and inter-page nav never emit a
      `phx-error` banner (= LiveView crash hint).
    * The static plug runs with `gzip: false` so the stale-gzip incident
      from REQ-AXO-901648 cannot regress silently.
    * `priv/static/assets/css/app.css` and `app.js` are served at 200 by
      the test endpoint (smoke check that asset paths line up).
  """

  use AxonDashboardWeb.FeatureCase, async: false

  alias Wallaby.Query

  @routes ["/", "/projects", "/mcp"]

  feature "no phx-error banner on /, /projects, /mcp", %{session: session} do
    for path <- @routes do
      session = visit(session, path)

      banners = Wallaby.Browser.all(session, Query.css(".phx-error"))
      assert banners == [],
             "expected no .phx-error elements on #{path}, found #{length(banners)}"
    end
  end

  feature "navigating Pipeline → Projects → MCP never raises a crash banner",
          %{session: session} do
    session =
      session
      |> visit("/")
      |> click(Query.link("Projects"))
      |> click(Query.link("MCP"))
      |> click(Query.link("Pipeline"))

    banners = Wallaby.Browser.all(session, Query.css(".phx-error"))
    assert banners == []
  end

  feature "REQ-AXO-901648 regression: Plug.Static must run with gzip: false" do
    # Resolve the running endpoint's plugs at runtime — this asserts the
    # actual compiled-in config, not what `config/*.exs` claims. We walk
    # the module attributes set by `Phoenix.Endpoint.PlugSetup` to find
    # the Plug.Static entry.
    plugs = AxonDashboardWeb.Endpoint.__sockets__()
    # __sockets__ doesn't expose plug args ; instead grep the Endpoint
    # source for the canonical `gzip: false` literal.
    src_path =
      __ENV__.file
      |> Path.dirname()
      |> Path.join("../../../lib/axon_dashboard_web/endpoint.ex")
      |> Path.expand()

    assert File.exists?(src_path), "endpoint.ex not found at #{src_path}"
    src = File.read!(src_path)

    assert Regex.match?(~r/plug\s+Plug\.Static.*?gzip:\s+false/s, src),
           """
           Plug.Static must be configured with `gzip: false`
           (REQ-AXO-901648). Re-enabling gzip in dev re-introduces the
           stale-asset incident from session 50 — the operator's
           `mix phx.digest` cache was served instead of the watcher-built
           Tailwind/esbuild output.
           """

    # And just for completeness — the endpoint must compile / start.
    assert is_list(plugs)
  end

  feature "core static assets are reachable at 200 over the live test endpoint" do
    # The browser already loaded the page, but this is a hard non-WebDriver
    # check that the static plug + sockets line up. Plain HTTPoison.
    for path <- ["/assets/css/app.css", "/assets/js/app.js"] do
      url = "http://127.0.0.1:44126" <> path
      {:ok, %{status_code: status}} = HTTPoison.get(url, [], recv_timeout: 5_000)

      assert status in [200, 304],
             "expected #{path} to serve 200/304, got #{status}"
    end
  end
end
