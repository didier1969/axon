defmodule Axon.BackpressureController do
  @moduledoc """
  Observes system load (CPU, RAM, IO) and adjusts Oban queues (acting as a Circuit Breaker).
  If any resource exceeds its configurable hard limit, it pauses Oban queues.
  Otherwise, it resumes and dynamically scales limits based on the pressure ratio.
  """
  use GenServer
  require Logger

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  def get_chunk_size(monitor_mod \\ Axon.ResourceMonitor) do
    load = monitor_mod.get_system_load()
    pressure = compute_pressure(load)

    cond do
      pressure < 0.50 -> 100
      pressure < 0.75 -> 50
      pressure < 1.00 -> 10
      true -> 5
    end
  end

  @impl true
  def init(opts) do
    poll_interval = Keyword.get(opts, :poll_interval, 2_000)
    monitor_mod = Keyword.get(opts, :monitor_mod, Axon.ResourceMonitor)
    oban_mod = Keyword.get(opts, :oban_mod, Oban)

    state = %{
      paused: false,
      last_limit: nil,
      poll_interval: poll_interval,
      monitor_mod: monitor_mod,
      oban_mod: oban_mod
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
    # Nexus Grade: Strict 40% cap to remain invisible to the user
    cpu_limit = Keyword.get(config, :cpu_hard_limit, 40.0)
    ram_limit = Keyword.get(config, :ram_hard_limit, 40.0)
    io_limit = Keyword.get(config, :io_hard_limit, 10.0)
    {cpu_limit, ram_limit, io_limit}
  end

  def compute_pressure(load) do
    {cpu_limit, ram_limit, io_limit} = get_limits()
    
    cpu_pressure = load.cpu / max(cpu_limit, 0.1)
    ram_pressure = load.ram / max(ram_limit, 0.1)
    io_pressure = Map.get(load, :io, 0.0) / max(io_limit, 0.1)
    
    pressure = max(cpu_pressure, max(ram_pressure, io_pressure))

    :telemetry.execute([:axon, :backpressure, :pressure_computed], %{pressure: pressure}, %{
      cpu: load.cpu,
      ram: load.ram,
      io: Map.get(load, :io, 0.0)
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
            "System resources saturated (Pressure: #{Float.round(pressure * 100, 1)}%). Pausing indexing queues. (CPU: #{Float.round(load.cpu, 1)}%, RAM: #{Float.round(load.ram, 1)}%, IO Wait: #{Float.round(Map.get(load, :io, 0.0), 1)}%)"
          )
          :telemetry.execute([:axon, :backpressure, :queues_paused], %{pressure: pressure})
          pause_queues(state.oban_mod)
        end
        %{state | paused: true, last_limit: 0}

      true ->
        if state.paused do
          Logger.info(
            "System load recovered (Pressure: #{Float.round(pressure * 100, 1)}%). Resuming indexing queues."
          )
          :telemetry.execute([:axon, :backpressure, :queues_resumed], %{pressure: pressure})
          resume_queues(state.oban_mod)
        end

        limit = calculate_limit(pressure)

        if state.last_limit != limit do
          Logger.info(
            "Adjusting indexing_default limit to #{limit} (Pressure: #{Float.round(pressure * 100, 1)}%)"
          )
          :telemetry.execute([:axon, :backpressure, :limit_adjusted], %{limit: limit}, %{pressure: pressure})
          scale_queues(state.oban_mod, limit)
        end

        %{state | paused: false, last_limit: limit}
    end
  end

  defp calculate_limit(pressure) do
    cond do
      pressure < 0.50 -> 10
      pressure < 0.75 -> 5
      true -> 1
    end
  end

  defp pause_queues(oban_mod) do
    oban_mod.pause_queue(queue: :indexing_default)
    oban_mod.pause_queue(queue: :indexing_hot)
  end

  defp resume_queues(oban_mod) do
    oban_mod.resume_queue(queue: :indexing_default)
    oban_mod.resume_queue(queue: :indexing_hot)
  end

  defp scale_queues(oban_mod, limit) do
    oban_mod.scale_queue(queue: :indexing_default, limit: limit)
    oban_mod.scale_queue(queue: :indexing_hot, limit: max(1, div(limit, 2)))
  end
end
