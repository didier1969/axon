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
    _ = :cpu_sup.util([:detailed])
    state = %{cpu: 0.0, ram: 0.0, io: 0.0}
    schedule_poll()
    {:ok, state}
  end

  @impl true
  def handle_call(:get_system_load, _from, state) do
    {:reply, state, state}
  end

  @impl true
  def handle_info(:poll, _state) do
    {cpu, io} = get_cpu_and_io()
    
    new_state = %{
      cpu: cpu,
      ram: get_ram(),
      io: io
    }

    schedule_poll()
    {:noreply, new_state}
  end

  defp schedule_poll do
    Process.send_after(self(), :poll, @poll_interval)
  end

  defp get_cpu_and_io do
    try do
      result = :cpu_sup.util([:detailed])
      
      # result on Linux is often {NumList, List1, List2, []}
      # We flatten all elements that are lists to search for the :wait key
      flattened = 
        result 
        |> Tuple.to_list() 
        |> Enum.filter(&is_list/1) 
        |> List.flatten()

      # We can also just get the basic util for overall cpu
      total_cpu = 
        case :cpu_sup.util() do
          cpu when is_number(cpu) -> cpu
          _ -> 0.0
        end
        
      # IO Wait is usually in the detailed list as :wait
      # If not found, default to 0.0
      io_wait = Keyword.get(flattened, :wait, 0.0)
      
      {total_cpu, io_wait}
    rescue
      _ -> {0.0, 0.0}
    end
  end

  defp get_ram do
    mem_data = :memsup.get_system_memory_data()

    total =
      Keyword.get(mem_data, :system_total_memory) || Keyword.get(mem_data, :total_memory) || 1

    free = Keyword.get(mem_data, :free_memory, 0)
    buffered = Keyword.get(mem_data, :buffered_memory, 0)
    cached = Keyword.get(mem_data, :cached_memory, 0)

    available = Keyword.get(mem_data, :available_memory, free + buffered + cached)
    used = total - available

    if total > 0 do
      used / total * 100.0
    else
      0.0
    end
  end
end
