defmodule Axon.Watcher.PoolFacade do
  @moduledoc """
  v2 Bridge Facade.
  Connects Pod A (Watcher) to Pod B (Axon Core Rust) via Unix Domain Socket.
  Replaces the old Python worker pool with a direct high-performance Rust bridge.
  """
  use GenServer
  require Logger

  @socket_path "/tmp/axon-v2.sock"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Sends a single file to Axon Core for parsing and ingestion.
  """
  def parse(path, content) do
    GenServer.call(__MODULE__, {:parse, path, content}, 30_000)
  end

  @doc """
  Sends a telemetry event to the Dashboard via the Rust Bridge.
  """
  def broadcast_event(type, payload) do
    GenServer.cast(__MODULE__, {:broadcast, type, payload})
  end

  # --- Callbacks ---

  def init(_opts) do
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, requests: %{}}}
  end

  def handle_cast({:broadcast, type, payload}, state) do
    if state.socket do
      event_json = Jason.encode!(%{type: type, payload: payload})
      :gen_tcp.send(state.socket, "WATCHER_EVENT #{event_json}\n")
    end

    {:noreply, state}
  end

  def handle_call({:parse, path, content}, _from, state) do
    if state.socket do
      payload = Jason.encode!(%{"path" => path, "content" => content})
      # We use a protocol: "PARSE_FILE <json_payload>\n"
      case :gen_tcp.send(state.socket, "PARSE_FILE #{payload}\n") do
        :ok ->
          # We store the 'from' to reply when the bridge confirms indexing
          # In v2, confirmations are async, but we can simplify the wait here 
          # or return :ok immediately if we trust the buffer.
          # For consistency with IndexingWorker, we return immediately.
          {:reply, %{"status" => "ok"}, state}

        {:error, reason} ->
          {:reply, {:error, reason}, state}
      end
    else
      {:reply, {:error, :not_connected}, state}
    end
  end

  def handle_info(:connect, state) do
    case :gen_tcp.connect({:local, @socket_path}, 0, [:binary, active: true]) do
      {:ok, socket} ->
        Logger.info("[Pod A] Connected to Axon Core Bridge (v2)")
        {:noreply, %{state | socket: socket}}

      {:error, _reason} ->
        Process.send_after(self(), :connect, 2000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, _socket, data}, state) do
    # Here we could handle acknowledgments from Rust
    # But for the Priority Scanner, we primarily want to push data.
    Logger.debug("[Pod A] Received from Bridge: #{inspect(data)}")

    data
    |> String.split("\n", trim: true)
    |> Enum.each(fn line ->
      case Jason.decode(line) do
        {:ok, %{"FileIndexed" => payload}} ->
          path = payload["path"]
          syms = payload["symbol_count"] || 0
          rels = payload["relation_count"] || 0
          sec = payload["security_score"] || 100
          cov = payload["coverage_score"] || 0
          entries = payload["entry_points"] || 0
          
          # Mettre à jour la base de données SQLite pour l'interface
          try do
            Axon.Watcher.Tracking.mark_file_status!(path, "indexed", %{
              symbols_count: syms,
              relations_count: rels,
              security_score: sec,
              coverage_score: cov,
              is_entry_point: entries > 0
            })
            
            # Mettre à jour le cache en mémoire pour éviter la charge SQLite
            project_id = Axon.Watcher.Tracking.get_project_id_for_file(path)
            if project_id do
               Axon.Watcher.StatsCache.increment_file_stats(project_id, %{
                 completed: 1,
                 symbols: syms,
                 relations: rels,
                 entries: entries,
                 security: sec,
                 coverage: cov
               })
            end
            
            # Publier l'évènement pour le LiveView
            Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:file_indexed, path, :ok})
          rescue
            e -> Logger.warning("[Pod A] Failed to update tracking for #{path}: #{inspect(e)}")
          end

        _ -> :ok
      end
    end)

    {:noreply, state}
  end

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("[Pod A] Bridge connection lost. Reconnecting...")
    send(self(), :connect)
    {:noreply, %{state | socket: nil}}
  end
end
