defmodule Axon.Watcher.PoolFacade do
  @moduledoc """
  Nexus v8.3 - Convergence Bridge.
  Telemetry still flows via Unix Socket (Full-Duplex).
  Analytics & Dashboard stats flow via HTTP SQL Gateway (Port 44129).
  """
  use GenServer
  require Logger

  @socket_path "/tmp/axon-telemetry.sock"
  @sql_gateway "http://127.0.0.1:44129/sql"

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
    # Direct Synchronous HTTP Request to Rust SQL Gateway
    headers = [{'content-type', 'application/json'}]
    body = Jason.encode!(%{"query" => query})
    
    case :httpc.request(:post, {to_charlist(@sql_gateway), headers, 'application/json', body}, [timeout: 5000], []) do
      {:ok, {{_version, 200, _reason}, _headers, response_body}} ->
        {:ok, List.to_string(response_body)}
      {:ok, {{_version, code, reason}, _headers, _body}} ->
        {:error, "HTTP #{code}: #{reason}"}
      {:error, reason} ->
        {:error, reason}
    end
  end

  # --- Callbacks ---

  def init(_opts) do
    Logger.info("[PoolFacade] IDENTITY PROBE: Nexus v8.3 (Convergence) Starting...")
    boot_id = Ecto.UUID.generate()
    Process.send_after(self(), :connect, 500)
    {:ok, %{socket: nil, requests: %{}, batches: %{}, boot_id: boot_id, buffer: ""}}
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
      {:noreply, %{state | batches: new_batches}}
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
    {lines, remaining} = split_lines(combined)

    new_state = Enum.reduce(lines, state, fn line, acc ->
      case Jason.decode(line) do
        {:ok, %{"FileIndexed" => payload}} -> process_indexed(payload, acc)
        {:ok, %{"event" => "PENDING_BATCH_READY", "files" => files}} -> process_pending(files, acc)
        {:ok, %{"event" => "BATCH_ACCEPTED"}} -> process_ack(acc)
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
    %{"batch" => files_to_index} |> Axon.Watcher.IndexingWorker.new() |> Oban.insert()
    state
  end

  defp process_indexed(p, state) do
    path = p["path"]
    final_status = if p["status"] == "ok", do: "indexed", else: p["status"]
    project_id = extract_project(path)

    if final_status == "indexed" do
      Axon.Watcher.StatsCache.increment_file_stats(project_id, %{completed: 1, symbols: p["symbol_count"] || 0, relations: p["relation_count"] || 0})
    end

    if p["t0"] > 0, do: Axon.Watcher.Tracer.record_trace(p["trace_id"] || "none", path, p["t0"], p["t1"] || 0, p["t2"] || 0, p["t3"] || 0, p["t4"] || 0)
    state
  end

  defp process_ack(state) do
    # Release waiting parse_batch calls
    if Map.has_key?(state, :batches) do
      Enum.each(state.batches, fn {_, {from, _, _}} -> 
        GenServer.reply(from, :ok)
      end)
      %{state | batches: %{}}
    else
      state
    end
  end

  defp extract_project(path) do
    case String.split(path, "/projects/") do
      [_, tail] -> String.split(tail, "/") |> List.first()
      _ -> "global"
    end
  end
end
