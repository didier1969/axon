defmodule AxonDashboard.BridgeClient do
  use GenServer
  require Logger

  @socket_path "/tmp/axon-v2.sock"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def trigger_scan do
    GenServer.cast(__MODULE__, :trigger_scan)
  end

  def stop_scan do
    GenServer.cast(__MODULE__, :stop_scan)
  end

  def reset_db do
    GenServer.cast(__MODULE__, :reset_db)
  end

  def get_state do
    GenServer.call(__MODULE__, :get_state)
  end

  def init(_opts) do
    Process.send_after(self(), :connect, 500)
    {:ok, %{
      socket: nil, 
      security_scores: %{}, 
      engine_start_time: nil, 
      engine_state: :idle # :idle | :indexing
    }}
  end

  def handle_call(:get_state, _from, state) do
    {:reply, state, state}
  end

  def handle_cast(:trigger_scan, state) do
    if state.socket != nil do
      Logger.info("[BRIDGE] Sending START command")
      :gen_tcp.send(state.socket, "START\n")
    end
    {:noreply, %{state | engine_state: :indexing}}
  end

  def handle_cast(:stop_scan, state) do
    if state.socket != nil do
      Logger.info("[BRIDGE] Sending STOP command")
      :gen_tcp.send(state.socket, "STOP\n")
    end
    {:noreply, %{state | engine_state: :idle}}
  end

  def handle_cast(:reset_db, state) do
    if state.socket != nil do
      Logger.info("[BRIDGE] Sending RESET command")
      :gen_tcp.send(state.socket, "RESET\n")
    end
    {:noreply, %{state | engine_state: :idle}}
  end

  def handle_info(:connect, state) do
    case :gen_tcp.connect({:local, @socket_path}, 0, [:binary, active: true]) do
      {:ok, socket} ->
        Logger.info("[BRIDGE] Connected to Data Plane")
        {:noreply, %{state | socket: socket}}

      {:error, _reason} ->
        Process.send_after(self(), :connect, 1000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, socket, data}, state) do
    # When Rust replies with Bridge Ready, we DO NOT automatically start index.
    # The LiveView will request it via user button, OR we could do it on first boot.
    # We will let it be manual to respect the user's "Job Interne" control logic.
    
    lines = String.split(data, "\n", trim: true)
    
    new_state = Enum.reduce(lines, state, fn line, acc ->
      if not String.contains?(line, "Axon Bridge Ready") do
        case Jason.decode(line) do
          {:ok, event} ->
            acc = handle_bridge_event(event, acc)
            Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})
            acc
          _ -> 
            acc
        end
      else
        acc
      end
    end)
    
    {:noreply, %{new_state | socket: socket}}
  end

  defp handle_bridge_event(%{"SystemReady" => %{"start_time_utc" => start_time}}, state) do
    case DateTime.from_iso8601(start_time) do
      {:ok, dt, _offset} -> %{state | engine_start_time: dt}
      _ -> state
    end
  end

  defp handle_bridge_event(%{"ScanStarted" => _}, state) do
    %{state | engine_state: :indexing}
  end

  defp handle_bridge_event(%{"ScanComplete" => _}, state) do
    %{state | engine_state: :idle}
  end

  defp handle_bridge_event(%{"FileIndexed" => payload}, state) do
    project = Map.get(payload, "path")
    new_score = Map.get(payload, "security_score", 100)
    
    if project && new_score > 0 do
      old_score = Map.get(state.security_scores, project, 100)
      
      if new_score < old_score do
        Logger.warning("[BRIDGE] Security Degraded for #{project}: #{old_score} -> #{new_score}")
        Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:security_degraded, project, old_score, new_score})
      end
      
      %{state | security_scores: Map.put(state.security_scores, project, new_score)}
    else
      state
    end
  end

  defp handle_bridge_event(%{"type" => "WatcherFileIndexed", "payload" => payload}, state) do
    path = payload["path"] || "unknown"
    status_str = payload["status"] || "unknown"
    status = if status_str == "ok", do: :ok, else: :error
    
    Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:file_indexed, path, status})
    state
  end

  defp handle_bridge_event(_, state), do: state

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("[BRIDGE] Connection lost. Reconnecting...")
    send(self(), :connect)
    {:noreply, %{state | socket: nil, engine_state: :idle}}
  end
end