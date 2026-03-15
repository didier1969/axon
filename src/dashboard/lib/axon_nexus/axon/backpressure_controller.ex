defmodule Axon.BackpressureController do
  @moduledoc """
  Observes system load and adjusts Oban queues (acting as a Circuit Breaker).
  If CPU or RAM > 40%, pauses Oban queues.
  Otherwise, resumes and dynamically scales limits based on available resources.
  """
  use GenServer
  require Logger

  @hard_limit 40.0

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  def get_chunk_size(monitor_mod \\ Axon.ResourceMonitor) do
    load = monitor_mod.get_system_load()
    max_load = max(load.cpu, load.ram)

    cond do
      max_load < 20.0 -> 100
      max_load < 30.0 -> 50
      max_load < 40.0 -> 10
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

  defp apply_backpressure(state) do
    load = state.monitor_mod.get_system_load()
    max_load = max(load.cpu, load.ram)

    cond do
      max_load >= @hard_limit ->
        if not state.paused do
          Logger.warning(
            "System load high (#{Float.round(max_load, 1)}% >= #{@hard_limit}%). Pausing indexing queues."
          )

          pause_queues(state.oban_mod)
        end

        %{state | paused: true, last_limit: 0}

      true ->
        if state.paused do
          Logger.info(
            "System load recovered (#{Float.round(max_load, 1)}% < #{@hard_limit}%). Resuming indexing queues."
          )

          resume_queues(state.oban_mod)
        end

        limit = calculate_limit(max_load)

        if state.last_limit != limit do
          Logger.info(
            "Adjusting indexing_default limit to #{limit} (load: #{Float.round(max_load, 1)}%)"
          )

          scale_queues(state.oban_mod, limit)
        end

        %{state | paused: false, last_limit: limit}
    end
  end

  defp calculate_limit(load) do
    cond do
      load < 20.0 -> 10
      load < 30.0 -> 5
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
