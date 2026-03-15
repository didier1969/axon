defmodule Axon.ResourceMonitor do
  @moduledoc """
  Polls OS metrics via :os_mon (:cpu_sup and :memsup) and caches them in GenServer state.
  """
  use GenServer

  @poll_interval 1_000

  # Client API
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def get_system_load do
    GenServer.call(__MODULE__, :get_system_load)
  end

  # Server Callbacks
  @impl true
  def init(_opts) do
    # Initial call to clear garbage value per Erlang docs
    _ = :cpu_sup.util()
    state = %{cpu: 0.0, ram: 0.0}
    schedule_poll()
    {:ok, state}
  end

  @impl true
  def handle_call(:get_system_load, _from, state) do
    {:reply, state, state}
  end

  @impl true
  def handle_info(:poll, _state) do
    new_state = %{
      cpu: get_cpu(),
      ram: get_ram()
    }
    schedule_poll()
    {:noreply, new_state}
  end

  defp schedule_poll do
    Process.send_after(self(), :poll, @poll_interval)
  end

  defp get_cpu do
    case :cpu_sup.util() do
      cpu when is_number(cpu) ->
        cpu
      _ ->
        0.0
    end
  end

  defp get_ram do
    mem_data = :memsup.get_system_memory_data()
    total = Keyword.get(mem_data, :system_total_memory) || Keyword.get(mem_data, :total_memory) || 1
    free = Keyword.get(mem_data, :free_memory, 0)
    buffered = Keyword.get(mem_data, :buffered_memory, 0)
    cached = Keyword.get(mem_data, :cached_memory, 0)

    available = Keyword.get(mem_data, :available_memory, free + buffered + cached)
    used = total - available

    if total > 0 do
      (used / total) * 100.0
    else
      0.0
    end
  end
end
