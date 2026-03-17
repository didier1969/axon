defmodule LiveView.Witness.Oracle do
  @moduledoc """
  A Plug to handle Out-of-Bound diagnostics (500s, Disconnects).
  """
  import Plug.Conn
  require Logger

  def init(opts), do: opts

  def call(%Plug.Conn{path_info: ["liveview_witness", "diagnose"]} = conn, _opts) do
    {:ok, body, conn} = read_body(conn)
    
    # Log the received diagnostic alert
    Logger.error("LiveView.Witness.Oracle received diagnostic: #{body}")
    
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(200, Jason.encode!(%{status: "received"}))
    |> halt()
  end

  def call(conn, _opts), do: conn
end
