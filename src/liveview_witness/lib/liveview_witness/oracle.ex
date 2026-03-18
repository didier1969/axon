defmodule LiveView.Witness.Oracle do
  @moduledoc """
  A Plug to handle Out-of-Bound diagnostics (500s, Disconnects).
  """
  import Plug.Conn
  require Logger

  def init(opts), do: opts

  def call(%Plug.Conn{path_info: ["liveview_witness", "diagnose"]} = conn, opts) do
    case get_req_header(conn, "x-witness-token") do
      [token] ->
        if LiveView.Witness.Token.verify?(token) do
          process_diagnostic(conn, opts)
        else
          unauthorized(conn)
        end

      _ ->
        unauthorized(conn)
    end
  end

  def call(conn, _opts), do: conn

  defp process_diagnostic(conn, opts) do
    {:ok, body, conn} = read_body(conn)
    pubsub = Keyword.get(opts, :pubsub, LiveView.Witness.pubsub())
    
    # Log the received diagnostic alert
    Logger.error("LiveView.Witness.Oracle received diagnostic: #{body}")
    
    # Broadcast the alert to the dashboard
    case Jason.decode(body) do
      {:ok, alert} ->
        :telemetry.execute([:liveview_witness, :health_alert, :received], %{count: 1}, %{
          type: Map.get(alert, "type"),
          watchdog: Map.get(alert, "watchdog"),
          url: Map.get(alert, "url")
        })

        Phoenix.PubSub.broadcast(pubsub, "witness_alerts", {:witness_alert, alert})

      _ ->
        Phoenix.PubSub.broadcast(pubsub, "witness_alerts", {:witness_alert, %{"message" => body}})
    end
    
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(200, Jason.encode!(%{status: "received"}))
    |> halt()
  end

  defp unauthorized(conn) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(401, Jason.encode!(%{error: "unauthorized"}))
    |> halt()
  end
end
