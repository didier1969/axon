defmodule Axon.Watcher.PoolFacade do
  @moduledoc """
  v2 Bridge Facade.
  Connects Pod A (Watcher) to Pod B (Axon Core Rust) via Unix Domain Socket.
  Replaces the old Python worker pool with a direct high-performance Rust bridge.
  """
  use GenServer
  require Logger

  @socket_path "/tmp/axon-telemetry.sock"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Sends a single file to Axon Core for parsing and ingestion.
  """
  def parse(path, lane \\ "fast", trace_id \\ "none", t0 \\ 0, t1 \\ 0) do
    GenServer.call(__MODULE__, {:parse, path, lane, trace_id, t0, t1}, 30_000)
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

  def handle_call({:parse, path, lane, trace_id, t0, t1}, from, state) do
    if state.socket do
      payload =
        Jason.encode!(%{
          "path" => path,
          "lane" => lane,
          "trace_id" => trace_id,
          "t0" => t0,
          "t1" => t1
        })

      # Protocol: "PARSE_FILE <json_payload>\n"
      # DECOUPLING: We send in a separate Task to avoid blocking the GenServer loop
      # if the socket buffer is full. This prevents deadlocks between send and receive.
      socket = state.socket
      Task.start(fn -> :gen_tcp.send(socket, "PARSE_FILE #{payload}\n") end)

      # Store the caller to reply when Rust confirms via TCP
      new_requests = Map.put(state.requests, path, from)
      {:noreply, %{state | requests: new_requests}}
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

    new_requests =
      data
      |> String.split("\n", trim: true)
      |> Enum.reduce(state.requests, fn line, acc_requests ->
        case Jason.decode(line) do
          {:ok, %{"FileIndexed" => payload}} ->
            path = payload["path"]
            status = payload["status"] || "ok"
            error_reason = payload["error_reason"] || ""
            syms = payload["symbol_count"] || 0
            rels = payload["relation_count"] || 0
            sec = payload["security_score"] || 100
            cov = payload["coverage_score"] || 0
            entries = payload["entry_points"] || 0

            # Telemetry Tracer Checkpoint T4 (Return to Elixir)
            t0 = payload["t0"] || 0
            t1 = payload["t1"] || 0
            t2 = payload["t2"] || 0
            t3 = payload["t3"] || 0
            t4 = payload["t4"] || 0
            trace_id = payload["trace_id"] || "none"

            if t0 > 0 and trace_id != "none" do
              Axon.Watcher.Tracer.record_trace(trace_id, path, t0, t1, t2, t3, t4)
            end

            # Reply to the caller
            case acc_requests[path] do
              from when not is_nil(from) ->
                GenServer.reply(from, %{
                  "status" => status,
                  "error_reason" => error_reason,
                  "symbols" => syms,
                  "relations" => rels,
                  "entries" => entries,
                  "sec" => sec,
                  "cov" => cov
                })

              _ ->
                :ok
            end

            Map.delete(acc_requests, path)

          _ ->
            acc_requests
        end
      end)

    {:noreply, %{state | requests: new_requests}}
  end

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("[Pod A] Bridge connection lost. Reconnecting...")
    send(self(), :connect)
    {:noreply, %{state | socket: nil}}
  end
end
