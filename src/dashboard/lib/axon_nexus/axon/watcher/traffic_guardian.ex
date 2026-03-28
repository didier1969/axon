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
    :timer.send_interval(100, :check_pressure)
    :timer.send_interval(1000, :calculate_flux)

    {:ok, %{
      target_pressure: 200,
      current_load: 0,
      t4_ema: 0.0,
      processed_this_sec: 0,
      total_processed: 0,
      last_activity: :os.system_time(:second)
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
    new_pressure = cond do
      new_ema > 200 ->
        max(@min_pressure, round(state.target_pressure * 0.8))
      new_ema < 50 ->
        min(@max_pressure, round(state.target_pressure * 1.1))
      true ->
        state.target_pressure
    end

    if new_pressure != state.target_pressure do
      Axon.Watcher.Telemetry.update_backpressure(new_pressure, new_ema)
      Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "telemetry_events", {:backpressure_update, %{pressure: new_pressure, t4_ema: new_ema}})
    end

    {:noreply, %{state | 
      t4_ema: new_ema, 
      target_pressure: new_pressure,
      current_load: max(0, state.current_load - 1),
      processed_this_sec: state.processed_this_sec + 1,
      total_processed: state.total_processed + 1,
      last_activity: :os.system_time(:second)
    }}
  end

  @impl true
  def handle_info(:calculate_flux, state) do
    # Logger.debug("[TrafficGuardian] Calculating flux: #{state.processed_this_sec} f/s")
    Axon.Watcher.Telemetry.update_flux(state.processed_this_sec * 1.0)
    {:noreply, %{state | processed_this_sec: 0}}
  end

  @impl true
  def handle_info(:check_pressure, state) do
    now = :os.system_time(:second)
    
    # Stall protection: If no activity for 15s but load > 0, reset load
    state = if now - state.last_activity > 15 and state.current_load > 0 do
      Logger.warning("[TrafficGuardian] Pipeline stall detected (No activity for 15s). Resetting current_load from #{state.current_load} to 0.")
      %{state | current_load: 0}
    else
      state
    end

    Logger.info("[TrafficGuardian] Checking pressure: load=#{state.current_load}, target=#{state.target_pressure}")
    # If current_load < (target_pressure / 2), call PoolFacade.pull_pending(target_pressure - current_load)
    if state.current_load < (state.target_pressure / 2) do
      to_pull = state.target_pressure - state.current_load
      
      if to_pull > 0 do
        Logger.info("[TrafficGuardian] Pulling #{to_pull} files from Rust...")
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
