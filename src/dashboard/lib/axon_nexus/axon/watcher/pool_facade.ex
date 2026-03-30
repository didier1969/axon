defmodule Axon.Watcher.PoolFacade do
  @moduledoc """
  Nexus v8.3 - Convergence Bridge.
  Telemetry still flows via Unix Socket (Full-Duplex).
  Analytics & Dashboard stats flow via HTTP SQL Gateway (Port 44129).
  """
  use GenServer
  require Logger

  @socket_path "/tmp/axon-telemetry.sock"
  # --- API ---

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def trigger_global_scan do
    GenServer.cast(__MODULE__, :trigger_global_scan)
  end

  def pull_pending(count) do
    GenServer.cast(__MODULE__, {:pull_pending, count})
  end

  def parse_batch(files) when is_list(files) do
    GenServer.call(__MODULE__, {:parse_batch, files}, 60_000)
  end

  def query_json(query) do
    Axon.Watcher.SqlGateway.query_json(query)
  end

  # --- Callbacks ---

  def init(_opts) do
    Logger.info("[PoolFacade] IDENTITY PROBE: Nexus v8.3 (Convergence) Starting...")
    boot_id = Ecto.UUID.generate()
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, requests: %{}, batches: %{}, boot_id: boot_id, buffer: ""}}
  end

  def handle_cast(:trigger_global_scan, state) do
    :telemetry.execute([:axon, :watcher, :scan_forwarded], %{count: 1}, %{
      connected: not is_nil(state.socket)
    })
    if state.socket, do: :gen_tcp.send(state.socket, "SCAN_ALL\n")
    {:noreply, state}
  end

  def handle_cast({:pull_pending, count}, state) do
    if state.socket, do: :gen_tcp.send(state.socket, "PULL_PENDING #{count}\n")
    {:noreply, state}
  end

  def handle_call({:parse_batch, files}, from, state) do
    if state.socket do
      batch_id = Ecto.UUID.generate()
      payload = Jason.encode!(%{"batch_id" => batch_id, "files" => files})

      case :gen_tcp.send(state.socket, "PARSE_BATCH #{payload}\n") do
        :ok ->
          new_batches = Map.put(state.batches, batch_id, {from, length(files), []})
          {:noreply, %{state | batches: new_batches}}

        {:error, reason} ->
          {:reply, {:error, reason}, state}
      end
    else
      {:reply, {:error, :not_connected}, state}
    end
  end

  def handle_call({:query_json, query}, _from, state) do
    # Fallback handle_call for legacy callers
    {:reply, query_json(query), state}
  end

  def handle_info(:connect, state) do
    case :gen_tcp.connect({:local, @socket_path}, 0, [:binary, active: true]) do
      {:ok, socket} ->
        Logger.info("[Pod A] Connected to Axon Core Telemetry (v8.3 Stable)")
        :gen_tcp.send(socket, "SESSION_INIT {\"boot_id\": \"#{state.boot_id}\"}\n")
        {:noreply, %{state | socket: socket, buffer: ""}}
      {:error, _} ->
        Process.send_after(self(), :connect, 2000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, _socket, data}, state) do
    combined = state.buffer <> data
    {lines, remaining} = Axon.Watcher.PoolProtocol.split_lines(combined)

    new_state = Enum.reduce(lines, state, fn line, acc ->
      case Jason.decode(line) do
        {:ok, %{"FileIndexed" => payload}} -> process_indexed(payload, acc)
        {:ok, %{"event" => "PENDING_BATCH_READY", "files" => files}} -> process_pending(files, acc)
        {:ok, %{"event" => "BATCH_ACCEPTED", "batch_id" => batch_id}} -> process_ack(acc, batch_id)
        {:ok, %{"event" => "BATCH_ACCEPTED"}} -> process_ack(acc, nil)
        _ -> acc
      end
    end)

    {:noreply, %{new_state | buffer: remaining}}
  end

  def handle_info({:tcp_closed, _}, state) do
    send(self(), :connect)
    {:noreply, %{state | socket: nil}}
  end

  # --- Internal Helpers ---

  defp process_pending(batch_files, state) do
    Axon.Watcher.PoolEventHandler.process_pending(batch_files)
    state
  end

  defp process_indexed(p, state) do
    Axon.Watcher.PoolEventHandler.process_indexed(p)
    state
  end

  defp process_ack(state, batch_id) do
    case Axon.Watcher.PoolProtocol.ack_targets(state.batches, batch_id) do
      [] ->
        state

      targets ->
        Enum.each(targets, fn {_id, {from, _, _}} ->
          GenServer.reply(from, :ok)
        end)

        remaining =
          Enum.reduce(targets, state.batches, fn {id, _batch}, batches ->
            Map.delete(batches, id)
          end)

        %{state | batches: remaining}
    end
  end
end
