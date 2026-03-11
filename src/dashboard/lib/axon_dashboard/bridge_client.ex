defmodule AxonDashboard.BridgeClient do
  use GenServer
  require Logger

  @socket_path "/tmp/axon-v2.sock"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def init(_opts) do
    send(self(), :connect)
    {:ok, %{socket: nil}}
  end

  def handle_info(:connect, state) do
    case :gen_tcp.connect({:local, @socket_path}, 0, [:binary, active: true]) do
      {:ok, socket} ->
        Logger.info("Connected to Axon Bridge at #{@socket_path}")
        {:noreply, %{state | socket: socket}}

      {:error, reason} ->
        Logger.debug("Could not connect to Axon Bridge: #{inspect(reason)}. Retrying in 2s...")
        Process.send_after(self(), :connect, 2000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, _socket, data}, state) do
    # Pour le moment, le noyau Rust envoie des strings de log ou du MsgPack
    # On va tenter de décoder en MsgPack, sinon on logue en tant que texte
    case Msgpax.unpack(data) do
      {:ok, event} ->
        Logger.debug("Bridge Event: #{inspect(event)}")
        Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})

      _ ->
        Logger.debug("Bridge Raw: #{String.trim(data)}")
    end

    {:noreply, state}
  end

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("Axon Bridge connection closed. Reconnecting...")
    send(self(), :connect)
    {:noreply, %{state | socket: nil}}
  end
end
