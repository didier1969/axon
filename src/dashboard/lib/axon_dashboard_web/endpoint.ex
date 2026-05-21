defmodule AxonDashboardWeb.Endpoint do
  use Phoenix.Endpoint, otp_app: :axon_dashboard

  # The session will be stored in the cookie and signed,
  # this means its contents can be read but not tampered with.
  # Set :encryption_salt if you would also like to encrypt it.
  @session_options [
    store: :cookie,
    key: "_axon_dashboard_key",
    signing_salt: "93Im2s7S",
    same_site: "Lax"
  ]

  socket "/live", Phoenix.LiveView.Socket,
    websocket: [connect_info: [session: @session_options]],
    longpoll: false

  # REQ-AXO-901648 : `gzip: not code_reloading?` evaluates to `gzip: true` in
  # this project because dev.exs sets `code_reloader: false` (operator opts
  # out of Phoenix code reloading to avoid devenv shell churn). The previous
  # logic then prefers stale `priv/static/assets/*.gz` files left over from a
  # past `mix phx.digest` run, masking the live Tailwind / esbuild watcher
  # output (incident session 50 : dashboard shipped unstyled). Hard-disable
  # gzip ; production builds re-enable it explicitly via `config/prod.exs`
  # after a fresh `mix phx.digest`.
  plug Plug.Static,
    at: "/",
    from: :axon_dashboard,
    gzip: false,
    only: AxonDashboardWeb.static_paths(),
    raise_on_missing_only: code_reloading?

  # Code reloading can be explicitly enabled under the
  # :code_reloader configuration of your endpoint.
  if code_reloading? do
    # socket "/phoenix/live_reload/socket", Phoenix.LiveReloader.Socket
    # plug Phoenix.LiveReloader
    plug Phoenix.CodeReloader
  end

  plug Plug.RequestId
  plug Plug.Telemetry, event_prefix: [:phoenix, :endpoint]

  plug LiveView.Witness.Oracle

  plug Plug.Parsers,
    parsers: [:urlencoded, :multipart, :json],
    pass: ["*/*"],
    json_decoder: Phoenix.json_library()

  plug Plug.MethodOverride
  plug Plug.Head
  plug Plug.Session, @session_options
  plug AxonDashboardWeb.Router
end
