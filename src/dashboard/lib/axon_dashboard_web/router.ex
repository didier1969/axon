defmodule AxonDashboardWeb.Router do
  use AxonDashboardWeb, :router

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {AxonDashboardWeb.Layouts, :root}
    plug :protect_from_forgery
    plug :put_secure_browser_headers
  end

  pipeline :api do
    plug :accepts, ["json"]
  end

  # REQ-AXO-901647: new 3-page cockpit (pipeline / projects / mcp).
  # Old single-page Axon.Watcher.CockpitLive is kept reachable at /legacy
  # for one session's worth of comparison, then will be retired.
  scope "/", AxonDashboardWeb.Live do
    pipe_through :browser

    live "/", PipelineLive, :index
    live "/cockpit", PipelineLive, :index
    live "/projects", ProjectsLive, :index
    live "/mcp", McpLive, :index
  end

  scope "/legacy", Axon.Watcher do
    pipe_through :browser
    live "/", CockpitLive, :index
  end

  # Other scopes may use custom stacks.
  # scope "/api", AxonDashboardWeb do
  #   pipe_through :api
  # end
end
