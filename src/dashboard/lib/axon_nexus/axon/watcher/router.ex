defmodule Axon.Watcher.Router do
  use Phoenix.Router
  import Phoenix.LiveView.Router

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :put_root_layout, html: {Axon.Watcher.Layouts, :root}
  end

  scope "/" do
    pipe_through :browser
    get "/favicon.ico", Axon.Watcher.FaviconController, :none
    live "/cockpit", Axon.Watcher.CockpitLive, :index
  end
end

defmodule Axon.Watcher.FaviconController do
  use Phoenix.Controller, formats: [:html, :json]
  def none(conn, _params), do: send_resp(conn, 204, "")
end

defmodule Axon.Watcher.Endpoint do
  use Phoenix.Endpoint, otp_app: :axon_watcher

  socket "/live", Phoenix.LiveView.Socket

  plug Plug.Parsers,
    parsers: [:urlencoded, :multipart, :json],
    pass: ["*/*"],
    json_decoder: Phoenix.json_library()

  plug Plug.MethodOverride
  plug Plug.Head

  plug Plug.Session,
    store: :cookie,
    key: "_axon_watcher_key",
    signing_salt: "axon_salt"

  plug Axon.Watcher.Router
end
