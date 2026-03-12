defmodule AxonDashboard.BridgeClient do
  use GenServer
  require Logger

  @socket_path "/tmp/axon-v2.sock"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def init(_opts) do
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil}}
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
    if is_binary(data) and String.contains?(data, "Axon Bridge Ready") do
       # On envoie le signal pour dire au Rust de commencer le scan
       if socket != nil do
         :gen_tcp.send(socket, "START\n")
       end
       {:noreply, %{state | socket: socket}}
    else
      case Msgpax.unpack(data) do
        {:ok, event} ->
          Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})
        _ -> 
          :ok
      end
      {:noreply, %{state | socket: socket}}
    end
  end

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("[BRIDGE] Connection lost. Reconnecting...")
    send(self(), :connect)
    {:noreply, %{state | socket: nil}}
  end
end
