defmodule Axon.Watcher.TrafficGuardian do
  @moduledoc """
  The Traffic Guardian Engine (Elixir side).
  Implements the sliding window logic and adaptive pressure control
  to prevent DuckDB/Rust backpressure from overwhelming the system.
  """
  use GenServer
  require Logger

  @alpha 0.2
  @min_pressure 10
  @max_pressure 1000

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    # Subscribe to telemetry events
    :telemetry.detach("traffic-guardian-handler")
    :telemetry.attach(
      "traffic-guardian-handler",
      [:axon, :watcher, :file_indexed],
      &__MODULE__.handle_telemetry/4,
      nil
    )

    # Start periodic check
    :timer.send_interval(2000, :check_pressure)

    {:ok, %{
      target_pressure: 100,
      current_load: 0,
      t4_ema: 0.0
    }}
  end

  # Telemetry callback (runs in the process that emits the event)
  def handle_telemetry(_event, measurements, _metadata, _config) do
    GenServer.cast(__MODULE__, {:file_indexed, measurements.t4})
  end

  @impl true
  def handle_cast({:file_indexed, t4}, state) do
    # Update EMA: EMA = alpha * T4 + (1 - alpha) * previous_EMA
    new_ema = if state.t4_ema == 0.0 do
      t4
    else
      @alpha * t4 + (1.0 - @alpha) * state.t4_ema
    end

    # Adjust target pressure based on EMA
    # - If T4 > 200ms -> Decrease pressure by 20% (min 10).
    # - If T4 < 50ms -> Increase pressure by 10% (max 1000).
    new_pressure = cond do
      new_ema > 200 ->
        max(@min_pressure, round(state.target_pressure * 0.8))
      new_ema < 50 ->
        min(@max_pressure, round(state.target_pressure * 1.1))
      true ->
        state.target_pressure
    end

    if new_pressure != state.target_pressure do
      Logger.info("[TrafficGuardian] Adjusting target pressure: #{state.target_pressure} -> #{new_pressure} (T4 EMA: #{Float.round(new_ema, 2)}ms)")
      Axon.Watcher.Telemetry.update_backpressure(new_pressure, new_ema)
      Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "telemetry_events", {:backpressure_update, %{pressure: new_pressure, t4_ema: new_ema}})
    end

    {:noreply, %{state | 
      t4_ema: new_ema, 
      target_pressure: new_pressure,
      current_load: max(0, state.current_load - 1)
    }}
  end

  @impl true
  def handle_info(:check_pressure, state) do
    # If current_load < (target_pressure / 2), call PoolFacade.pull_pending(target_pressure - current_load)
    if state.current_load < (state.target_pressure / 2) do
      to_pull = state.target_pressure - state.current_load
      
      if to_pull > 0 do
        # Logger.debug("[TrafficGuardian] Pulling #{to_pull} files (Current load: #{state.current_load}, Target: #{state.target_pressure})")
        Axon.Watcher.PoolFacade.pull_pending(to_pull)
        {:noreply, %{state | current_load: state.current_load + to_pull}}
      else
        {:noreply, state}
      end
    else
      {:noreply, state}
    end
  end
end
