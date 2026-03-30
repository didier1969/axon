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
    state = %{cpu: 0.0, ram: 0.0, io: 0.0, io_prev: read_proc_stat()}
    schedule_poll()
    {:ok, state}
  end

  @impl true
  def handle_call(:get_system_load, _from, state) do
    {:reply, state, state}
  end

  @impl true
  def handle_info(:poll, state) do
    {cpu, io, io_prev} = get_cpu_and_io(state.io_prev)

    new_state = %{
      cpu: cpu,
      ram: get_ram(),
      io: io,
      io_prev: io_prev
    }

    schedule_poll()
    {:noreply, new_state}
  end

  defp schedule_poll do
    Process.send_after(self(), :poll, @poll_interval)
  end

  defp get_cpu_and_io(previous_io) do
    try do
      # `:cpu_sup.util()` safely returns the total CPU usage as a float percentage.
      total_cpu = :cpu_sup.util()
      current_io = read_proc_stat()
      io = io_wait_percent(previous_io, current_io)
      {total_cpu, io, current_io}
    rescue
      _ -> {0.0, 0.0, previous_io}
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

  defp read_proc_stat do
    with {:ok, content} <- File.read("/proc/stat"),
         ["cpu" | values] <- content |> String.split("\n", trim: true) |> List.first() |> String.split(),
         parsed <- Enum.map(values, &String.to_integer/1),
         true <- length(parsed) >= 5 do
      idle = Enum.at(parsed, 3, 0)
      iowait = Enum.at(parsed, 4, 0)
      total = Enum.sum(parsed)
      %{idle: idle, iowait: iowait, total: total}
    else
      _ -> nil
    end
  rescue
    _ -> nil
  end

  defp io_wait_percent(nil, _current), do: 0.0
  defp io_wait_percent(_previous, nil), do: 0.0

  defp io_wait_percent(previous, current) do
    total_delta = max(current.total - previous.total, 0)
    iowait_delta = max(current.iowait - previous.iowait, 0)

    if total_delta > 0 do
      iowait_delta / total_delta * 100.0
    else
      0.0
    end
  end
end
