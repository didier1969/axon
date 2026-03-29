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

  scope "/", Axon.Watcher do
    pipe_through :browser
    live "/", CockpitLive, :index
    live "/cockpit", CockpitLive, :index
  end

  # Other scopes may use custom stacks.
  # scope "/api", AxonDashboardWeb do
  #   pipe_through :api
  # end
end
