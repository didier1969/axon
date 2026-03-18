defmodule LiveView.Witness.Oracle do
  @moduledoc """
  A Plug to handle Out-of-Bound diagnostics (500s, Disconnects).
  """
  import Plug.Conn
  require Logger

  def init(opts), do: opts

  def call(%Plug.Conn{path_info: ["liveview_witness", "diagnose"]} = conn, opts) do
    {:ok, body, conn} = read_body(conn)
    pubsub = Keyword.get(opts, :pubsub, LiveView.Witness.PubSub)
    
    # Log the received diagnostic alert
    Logger.error("LiveView.Witness.Oracle received diagnostic: #{body}")
    
    # Broadcast the alert to the dashboard
    case Jason.decode(body) do
      {:ok, alert} ->
        :telemetry.execute([:liveview_witness, :health_alert, :received], %{}, %{
          type: Map.get(alert, "type"),
          watchdog: Map.get(alert, "watchdog")
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

  def call(conn, _opts), do: conn
end
