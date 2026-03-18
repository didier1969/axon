defmodule AxonDashboard.TelemetryHandler do
  @moduledoc """
  Bridges :telemetry events to Phoenix.PubSub for live dashboard updates.
  """
  use GenServer
  require Logger

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    attach_events()
    {:ok, %{}}
  end

  defp attach_events do
    :telemetry.attach_many(
      "axon-dashboard-handler",
      [
        [:axon, :backpressure, :pressure_computed],
        [:axon, :backpressure, :queues_paused],
        [:axon, :backpressure, :queues_resumed],
        [:axon, :backpressure, :limit_adjusted],
        [:axon, :watcher, :batch_enqueued],
        [:axon, :watcher, :batch_failed]
      ],
      &__MODULE__.handle_event/4,
      nil
    )
  end

  def handle_event(event, measurements, metadata, _config) do
    Phoenix.PubSub.broadcast(
      AxonDashboard.PubSub,
      "telemetry_events",
      {:telemetry_event, event, measurements, metadata}
    )
  end
end
