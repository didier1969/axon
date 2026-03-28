defmodule Axon.Watcher.PoolFacade do
  @moduledoc """
  Nexus v5.0 - Monolithic Stable Bridge.
  Restores the functional feedback loop between Elixir and Rust.
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

  # --- Callbacks ---

  def init(_opts) do
    Logger.info("[PoolFacade] IDENTITY PROBE: Nexus v5.0 (Monolith) Starting...")
    boot_id = Ecto.UUID.generate()
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, requests: %{}, batches: %{}, path_to_batch: %{}, boot_id: boot_id, buffer: ""}}
  end

  def handle_cast(:trigger_global_scan, state) do
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
      payload = Jason.encode!(files)
      
      :gen_tcp.send(state.socket, "PARSE_BATCH #{payload}\n")

      new_batches = Map.put(state.batches, batch_id, {from, length(files), []})
      new_path_to_batch = Enum.reduce(files, state.path_to_batch, fn f, acc -> Map.put(acc, f["path"], batch_id) end)
      
      {:noreply, %{state | batches: new_batches, path_to_batch: new_path_to_batch}}
    else
      {:reply, {:error, :not_connected}, state}
    end
  end

  def handle_info(:connect, state) do
    case :gen_tcp.connect({:local, @socket_path}, 0, [:binary, active: true]) do
      {:ok, socket} ->
        Logger.info("[Pod A] Connected to Axon Core Bridge (v5.0 Stable)")
        :gen_tcp.send(socket, "SESSION_INIT {\"boot_id\": \"#{state.boot_id}\"}\n")
        {:noreply, %{state | socket: socket, buffer: ""}}
      {:error, _} ->
        Process.send_after(self(), :connect, 2000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, _socket, data}, state) do
    combined = state.buffer <> data
    {lines, remaining} = split_lines(combined)

    new_state = Enum.reduce(lines, state, fn line, acc ->
      case Jason.decode(line) do
        {:ok, %{"FileIndexed" => payload}} -> process_indexed(payload, acc)
        {:ok, %{"event" => "PENDING_BATCH_READY", "files" => files}} -> process_pending(files, acc)
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

  defp split_lines(data) do
    if String.ends_with?(data, "\n") do
      {String.split(data, "\n", trim: true), ""}
    else
      parts = String.split(data, "\n")
      {Enum.slice(parts, 0..-2//1), List.last(parts)}
    end
  end

  defp process_pending(batch_files, state) do
    files_to_index = Enum.map(batch_files, fn f -> 
      %{"path" => f["path"], "trace_id" => f["trace_id"], "priority" => f["priority"] || 100}
    end)

    # Register in Tracking
    batch_files
    |> Enum.group_by(fn f -> extract_project(f["path"]) end)
    |> Enum.each(fn {proj, files} ->
      Axon.Watcher.Tracking.upsert_project!(proj, "workspace")
      Axon.Watcher.Tracking.upsert_files_batch!(proj, Enum.map(files, fn f -> {f["path"], 0, "pending"} end))
    end)

    # Launch Oban
    %{"batch" => files_to_index} |> Axon.Watcher.IndexingWorker.new() |> Oban.insert()
    state
  end

  defp process_indexed(p, state) do
    path = p["path"]
    final_status = if p["status"] == "ok", do: "indexed", else: p["status"]
    project_id = extract_project(path)

    # Update Tracking & Stats
    data = [{path, 0, final_status, p["symbol_count"] || 0, p["relation_count"] || 0, p["security_score"] || 100, p["coverage_score"] || 0, 0, 0, 0, p["error_reason"] || ""}]
    Axon.Watcher.Tracking.upsert_files_full_batch!(project_id, data)
    
    if final_status == "indexed" do
      Axon.Watcher.StatsCache.increment_file_stats(project_id, %{completed: 1, symbols: p["symbol_count"] || 0, relations: p["relation_count"] || 0})
    end

    # Tracer
    if p["t0"] > 0, do: Axon.Watcher.Tracer.record_trace(p["trace_id"] || "none", path, p["t0"], p["t1"] || 0, p["t2"] || 0, p["t3"] || 0, p["t4"] || 0)

    # IMPORTANT: Reply to GenServer calls (Fixes the Deadlock)
    case Map.pop(state.path_to_batch, path) do
      {nil, _} -> state
      {batch_id, new_p2b} ->
        case Map.get(state.batches, batch_id) do
          {from, 1, results} ->
            GenServer.reply(from, %{"status" => "ok", "results" => [%{"path" => path, "status" => final_status} | results]})
            %{state | batches: Map.delete(state.batches, batch_id), path_to_batch: new_p2b}
          {from, count, results} ->
            new_batches = Map.put(state.batches, batch_id, {from, count - 1, [%{"path" => path, "status" => final_status} | results]})
            %{state | batches: new_batches, path_to_batch: new_p2b}
          _ -> state
        end
    end
  end

  defp extract_project(path) do
    case String.split(path, "/projects/") do
      [_, tail] -> String.split(tail, "/") |> List.first()
      _ -> "global"
    end
  end
end
