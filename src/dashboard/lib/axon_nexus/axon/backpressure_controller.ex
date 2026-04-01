# Copyright (c) Didier Stadelmann. All rights reserved.
defmodule Axon.BackpressureController do
  @moduledoc """
  Observes system load (CPU, RAM, IO) and publishes pressure guidance for the UI.
  Rust owns canonical ingestion throttling; Elixir no longer pauses or scales queues.
  """
  use GenServer
  require Logger

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  @impl true
  def init(opts) do
    poll_interval = Keyword.get(opts, :poll_interval, 2_000)
    monitor_mod = Keyword.get(opts, :monitor_mod, Axon.ResourceMonitor)

    state = %{
      paused: false,
      last_limit: nil,
      poll_interval: poll_interval,
      monitor_mod: monitor_mod
    }

    if poll_interval > 0 do
      schedule_poll(poll_interval)
    end

    {:ok, state}
  end

  @impl true
  def handle_info(:poll, state) do
    state = apply_backpressure(state)

    if state.poll_interval > 0 do
      schedule_poll(state.poll_interval)
    end

    {:noreply, state}
  end

  # For testing without polling
  @impl true
  def handle_call(:trigger_poll, _from, state) do
    new_state = apply_backpressure(state)
    {:reply, :ok, new_state}
  end

  defp schedule_poll(interval) do
    Process.send_after(self(), :poll, interval)
  end

  def get_limits do
    config = Application.get_env(:axon_dashboard, Axon.BackpressureController, [])
    cpu_limit = Keyword.get(config, :cpu_hard_limit, 70.0)
    ram_limit = Keyword.get(config, :ram_hard_limit, 70.0)
    io_limit = Keyword.get(config, :io_hard_limit, 20.0)
    {cpu_limit, ram_limit, io_limit}
  end

  def compute_pressure(load) do
    {cpu_limit, ram_limit, io_limit} = get_limits()

    cpu_val = if is_number(load.cpu), do: load.cpu, else: 0.0
    ram_val = if is_number(load.ram), do: load.ram, else: 0.0
    io_val = if is_number(Map.get(load, :io, 0.0)), do: Map.get(load, :io, 0.0), else: 0.0

    cpu_pressure = cpu_val / max(cpu_limit, 0.1)
    ram_pressure = ram_val / max(ram_limit, 0.1)
    io_pressure = io_val / max(io_limit, 0.1)

    pressure = max(cpu_pressure, max(ram_pressure, io_pressure))

    :telemetry.execute([:axon, :backpressure, :pressure_computed], %{pressure: pressure}, %{
      cpu: cpu_val,
      ram: ram_val,
      io: io_val
    })

    pressure
  end

  defp apply_backpressure(state) do
    load = state.monitor_mod.get_system_load()
    pressure = compute_pressure(load)

    cond do
      pressure >= 1.0 ->
        if not state.paused do
          Logger.warning(
            "System resources saturated (Pressure: #{Float.round(pressure * 100, 1)}%). Publishing constrained state only. (CPU: #{Float.round(load.cpu, 1)}%, RAM: #{Float.round(load.ram, 1)}%, IO Wait: #{Float.round(Map.get(load, :io, 0.0), 1)}%)"
          )

          :telemetry.execute([:axon, :backpressure, :queues_paused], %{pressure: pressure})
        end

        %{state | paused: true, last_limit: 0}

      true ->
        if state.paused do
          Logger.info(
            "System load recovered (Pressure: #{Float.round(pressure * 100, 1)}%). Publishing unconstrained state only."
          )

          :telemetry.execute([:axon, :backpressure, :queues_resumed], %{pressure: pressure})
        end

        limit = calculate_limit(pressure)

        if state.last_limit != limit do
          Logger.info(
            "Adjusting Rust guidance limit to #{limit} (Pressure: #{Float.round(pressure * 100, 1)}%)"
          )

          :telemetry.execute([:axon, :backpressure, :limit_adjusted], %{limit: limit}, %{
            pressure: pressure
          })
        end

        %{state | paused: false, last_limit: limit}
    end
  end

  defp calculate_limit(pressure) do
    cond do
      pressure < 0.50 -> 16
      pressure < 0.75 -> 8
      true -> 2
    end
  end
end
