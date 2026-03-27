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
  Sends a batch of files to Axon Core for parsing.
  """
  def parse_batch(files) when is_list(files) do
    GenServer.call(__MODULE__, {:parse_batch, files}, 60_000)
  end

  @doc """
  Sends a telemetry event to the Dashboard via the Rust Bridge.
  """
  def broadcast_event(type, payload) do
    GenServer.cast(__MODULE__, {:broadcast, type, payload})
  end

  # --- Callbacks ---

  def init(_opts) do
    # Generate a unique session ID for this Elixir instance
    boot_id = Ecto.UUID.generate()
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, requests: %{}, batches: %{}, path_to_batch: %{}, boot_id: boot_id}}
  end

  # ... broadcast_event remains same ...

  def handle_call({:parse_batch, files}, from, state) do
    if state.socket do
      batch_id = Ecto.UUID.generate()
      payload = Jason.encode!(files)

      # Async send to avoid blocking the GenServer loop
      socket = state.socket
      Task.start(fn -> :gen_tcp.send(socket, "PARSE_BATCH #{payload}\n") end)

      # Track batch progress
      new_batches = Map.put(state.batches, batch_id, {from, length(files), []})
      
      new_path_to_batch = Enum.reduce(files, state.path_to_batch, fn file, acc ->
        Map.put(acc, file["path"], batch_id)
      end)

      {:noreply, %{state | batches: new_batches, path_to_batch: new_path_to_batch}}
    else
      {:reply, {:error, :not_connected}, state}
    end
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

      socket = state.socket
      Task.start(fn -> :gen_tcp.send(socket, "PARSE_FILE #{payload}\n") end)

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
        
        # HANDSHAKE: Inform Rust about our new session
        # This allows Rust to purge old tasks from previous crashed sessions.
        :gen_tcp.send(socket, "SESSION_INIT {\"boot_id\": \"#{state.boot_id}\"}\n")
        
        {:noreply, %{state | socket: socket}}

      {:error, _reason} ->
        Process.send_after(self(), :connect, 2000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, _socket, data}, state) do
    # Here we handle acknowledgments from Rust.
    # We batch them to avoid unit SQLite transactions.
    Logger.debug("[Pod A] Received from Bridge: #{byte_size(data)} bytes")

    lines = String.split(data, "\n", trim: true)
    
    # First, decode all events
    events = Enum.map(lines, fn line ->
      case Jason.decode(line) do
        {:ok, %{"FileIndexed" => payload}} -> payload
        _ -> nil
      end
    end) |> Enum.filter(& &1)

    if events != [] do
      Logger.info("[PoolFacade] Processing #{length(events)} feedback events from Rust.")
      
      # 1. Update Tracking in batch (Zero-SELECT path)
      events
      |> Enum.group_by(fn payload -> Axon.Watcher.Tracking.extract_project_from_path(payload["path"]) end)
      |> Enum.each(fn {project_id, project_events} ->
        data_for_upsert = Enum.map(project_events, fn p ->
          {
            p["path"], 
            0, # Hash already set by staging
            p["status"] || "ok",
            p["symbol_count"] || 0,
            p["relation_count"] || 0,
            p["security_score"] || 100,
            p["coverage_score"] || 0,
            0, # duration handled via traces
            0, # ram_b
            0  # ram_a
          }
        end)
        
        try do
          Axon.Watcher.Tracking.upsert_files_full_batch!(project_id, data_for_upsert)
        rescue
          e -> Logger.error("[PoolFacade] Batch Upsert failed: #{inspect(e)}")
        end
      end)

      # 2. Process feedback and handle replies (Batches and Unit)
      new_state = Enum.reduce(events, state, fn payload, acc_state ->
        path = payload["path"]
        status = payload["status"] || "ok"
        syms = payload["symbol_count"] || 0
        rels = payload["relation_count"] || 0
        sec = payload["security_score"] || 100
        cov = payload["coverage_score"] || 0
        entries = payload["entry_points"] || 0
        
        # Update Global Stats Cache for SUCCESSFUL files (Mem-only, fast)
        if status == "ok" do
          project_name = Axon.Watcher.Tracking.extract_project_from_path(path)
          Axon.Watcher.StatsCache.increment_file_stats(project_name, %{
            completed: 1,
            symbols: syms,
            relations: rels,
            entries: entries,
            security: sec,
            coverage: cov
          })
        end

        # Recording tracer data
        t0 = payload["t0"] || 0
        trace_id = payload["trace_id"] || "none"
        if t0 > 0 and trace_id != "none" do
          Axon.Watcher.Tracer.record_trace(trace_id, path, t0, payload["t1"] || 0, payload["t2"] || 0, payload["t3"] || 0, payload["t4"] || 0)
        end

        # A. Check if it's part of a batch
        case Map.pop(acc_state.path_to_batch, path) do
          {nil, _} ->
            # B. It's a unit request (legacy or Titan)
            case Map.pop(acc_state.requests, path) do
              {nil, _} -> acc_state
              {from, new_reqs} ->
                GenServer.reply(from, %{"status" => payload["status"] || "ok"})
                %{acc_state | requests: new_reqs}
            end

          {batch_id, new_p2b} ->
            # C. Handle Batch progress
            case Map.get(acc_state.batches, batch_id) do
              {from, pending, results} ->
                new_pending = pending - 1
                new_results = [%{"path" => path, "status" => payload["status"]} | results]
                
                if new_pending == 0 do
                  # Batch COMPLETE! Reply to Oban worker
                  GenServer.reply(from, %{"status" => "ok", "results" => new_results})
                  %{acc_state | batches: Map.delete(acc_state.batches, batch_id), path_to_batch: new_p2b}
                else
                  # Batch still waiting
                  %{acc_state | batches: Map.put(acc_state.batches, batch_id, {from, new_pending, new_results}), path_to_batch: new_p2b}
                end
              _ -> 
                %{acc_state | path_to_batch: new_p2b}
            end
        end
      end)

      {:noreply, new_state}
    else
      {:noreply, state}
    end
  end

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("[Pod A] Bridge connection lost. Reconnecting...")
    send(self(), :connect)
    {:noreply, %{state | socket: nil}}
  end
end
