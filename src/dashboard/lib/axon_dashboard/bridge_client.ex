# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboard.BridgeClient do
  use GenServer
  require Logger

  alias AxonDashboard.DashboardState

  @socket_path "/tmp/axon-telemetry.sock"

  @doc """
  REQ-AXO-901826 — dedicated PubSub topic for the
  `dashboard_state_v1` envelope. LiveViews subscribe to this topic
  specifically, not the broader `bridge_events` legacy topic (which
  still carries FileIndexed / SystemReady / ScanStarted etc.).
  """
  def dashboard_topic, do: "dashboard:state"

  @doc "Legacy topic for non-dashboard bridge events (FileIndexed, etc.)."
  def bridge_topic, do: "bridge_events"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def get_state do
    GenServer.call(__MODULE__, :get_state)
  end

  @doc """
  REQ-AXO-901806 — return the latest cached `%DashboardState{}` struct
  or `nil` if no event has arrived yet. LiveView mount callsites use
  this to pre-warm `@dashboard_state` without waiting for the first
  PubSub event after subscribe. `catch :exit` keeps mount resilient
  during a brain reconnect window.
  """
  @spec dashboard_state() :: DashboardState.t() | nil
  def dashboard_state do
    case GenServer.whereis(__MODULE__) do
      nil -> nil
      _pid -> safe_call(:dashboard_state)
    end
  end

  defp safe_call(msg) do
    GenServer.call(__MODULE__, msg, 500)
  catch
    :exit, _ -> nil
  end

  @doc """
  REQ-AXO-094 — send a single line-terminated command up the
  telemetry socket to the brain. Used by `BeamAlarmHandler` to push
  `BEAM_ALARM` events. If the socket is not currently connected,
  the command is dropped (the next reconnect will pick up the
  next-fired alarm; no queueing intended). Returns :ok on send,
  :error if no socket.
  """
  def send_command(line) when is_binary(line) do
    GenServer.cast(__MODULE__, {:send_command, line})
  end

  def init(_opts) do
    Process.send_after(self(), :connect, 500)

    {:ok,
     %{
       socket: nil,
       security_scores: %{},
       coverage_scores: %{},
       taint_paths: %{},
       engine_start_time: nil,
       # :idle | :indexing
       engine_state: :idle,
       # REQ-AXO-901806 — cache of the latest dashboard_state_v1 payload.
       # nil until first event arrives from brain.
       dashboard_state: nil,
       # REQ-AXO-901826 — TCP buffer accumulator. The brain socket emits
       # line-terminated JSON envelopes ; dashboard_state_v1 is ~5KB so
       # a single envelope WILL fragment across multiple `{:tcp, _, _}`
       # messages. Without a buffer, `Jason.decode/1` fails on partial
       # JSON and the LiveView never receives a typed event.
       buffer: ""
     }}
  end

  def handle_call(:get_state, _from, state) do
    {:reply, state, state}
  end

  def handle_call(:dashboard_state, _from, state) do
    {:reply, state.dashboard_state, state}
  end

  # REQ-AXO-094 — write a BEAM_ALARM (or any other line-terminated
  # command) to the brain telemetry socket. We append a trailing
  # newline because the brain's telemetry parser is line-based
  # (see `main_telemetry::handle_telemetry_command`).
  def handle_cast({:send_command, line}, %{socket: nil} = state) do
    Logger.warning(
      "[BRIDGE] send_command dropped: no socket connected; payload=#{String.slice(line, 0, 120)}"
    )
    {:noreply, state}
  end

  def handle_cast({:send_command, line}, %{socket: socket} = state) do
    payload = if String.ends_with?(line, "\n"), do: line, else: line <> "\n"
    case :gen_tcp.send(socket, payload) do
      :ok ->
        {:noreply, state}
      {:error, reason} ->
        Logger.warning("[BRIDGE] send_command failed: #{inspect(reason)}; will reconnect")
        send(self(), :connect)
        {:noreply, %{state | socket: nil}}
    end
  end

  def handle_cast(_message, state), do: {:noreply, state}

  def handle_info(:connect, state) do
    # REQ-AXO-901826 — read nested config key (config :axon_dashboard,
    # AxonDashboard.BridgeClient, telemetry_socket_path: ...) set by
    # config/runtime.exs per instance kind. Legacy top-level key kept
    # as fallback for test.exs which still writes it directly under
    # :axon_dashboard, telemetry_socket_path.
    socket_path =
      Application.get_env(:axon_dashboard, __MODULE__, [])
      |> Keyword.get(:telemetry_socket_path) ||
        Application.get_env(:axon_dashboard, :telemetry_socket_path, @socket_path)

    case :gen_tcp.connect({:local, socket_path}, 0, [:binary, active: true]) do
      {:ok, socket} ->
        Logger.info("[BRIDGE] Connected to Data Plane")
        Axon.Watcher.Telemetry.mark_bridge_connected()
        {:noreply, %{state | socket: socket}}

      {:error, _reason} ->
        Process.send_after(self(), :connect, 1000)
        {:noreply, state}
    end
  end

  def handle_info({:tcp, socket, data}, state) do
    # REQ-AXO-901826 — line-buffered framing. Accumulate the raw bytes
    # into `state.buffer`, split on `\n`, keep the trailing remainder
    # (which may be a partial envelope) for the next `:tcp` message.
    combined = state.buffer <> data
    {complete_lines, remainder} = split_complete_lines(combined)

    new_state =
      Enum.reduce(complete_lines, %{state | buffer: remainder}, fn line, acc ->
        line = String.trim(line)

        if line != "" and not String.contains?(line, "Axon Bridge Ready") do
          case Jason.decode(line) do
            # REQ-AXO-901806 — dashboard_state_v1 is the single-event
            # dashboard refresh payload. Emit a typed message so LiveViews
            # pattern-match without parsing `event["event"]` every tick.
            # Skip handle_bridge_event (no BridgeClient state impact).
            {:ok, %{"event" => "dashboard_state_v1"} = raw} ->
              # REQ-AXO-901826 idiomatic refactor :
              # (i) convert to typed %DashboardState{} (atom keys)
              # (ii) local_broadcast (in-VM, no cluster RPC)
              # (iii) dedicated topic `"dashboard:state"` so LiveViews
              #      pattern-match `{:dashboard_state, %DashboardState{}}`
              #      without seeing FileIndexed / ScanStarted etc.
              typed = DashboardState.from_map(raw)

              Phoenix.PubSub.local_broadcast(
                AxonDashboard.PubSub,
                dashboard_topic(),
                {:dashboard_state, typed}
              )

              %{acc | dashboard_state: typed}

            {:ok, event} ->
              acc = handle_bridge_event(event, acc)

              Phoenix.PubSub.local_broadcast(
                AxonDashboard.PubSub,
                bridge_topic(),
                {:bridge_event, event}
              )

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

  def handle_info({:tcp_closed, _socket}, state) do
    Logger.warning("[BRIDGE] Connection lost. Reconnecting...")
    Axon.Watcher.Telemetry.mark_bridge_disconnected()
    send(self(), :connect)
    {:noreply, %{state | socket: nil, engine_state: :idle, buffer: ""}}
  end

  # REQ-AXO-901826 — split accumulator on `\n`, return complete lines
  # and the trailing remainder (partial envelope waiting for more bytes).
  defp split_complete_lines(buffer) do
    case String.split(buffer, "\n") do
      [partial] -> {[], partial}
      pieces ->
        {complete, [partial]} = Enum.split(pieces, length(pieces) - 1)
        {complete, partial}
    end
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
    record_file_indexed(payload)

    project = Map.get(payload, "path")
    new_score = Map.get(payload, "security_score", 100)
    paths_json = Map.get(payload, "taint_paths", "[]")

    if project && new_score > 0 do
      old_score = Map.get(state.security_scores, project, 100)
      cov_score = Map.get(payload, "coverage_score", 0)

      if new_score < old_score do
        Logger.warning("[BRIDGE] Security Degraded for #{project}: #{old_score} -> #{new_score}")

        Phoenix.PubSub.local_broadcast(
          AxonDashboard.PubSub,
          bridge_topic(),
          {:security_degraded, project, old_score, new_score}
        )
      end

      state = %{
        state
        | security_scores: Map.put(state.security_scores, project, new_score),
          coverage_scores: Map.put(state.coverage_scores, project, cov_score)
      }

      paths =
        case Jason.decode(paths_json) do
          {:ok, p} -> p
          _ -> []
        end

      %{state | taint_paths: Map.put(state.taint_paths, project, paths)}
    else
      state
    end
  end

  defp handle_bridge_event(%{"RuntimeTelemetry" => payload}, state) do
    Axon.Watcher.Telemetry.update_runtime_telemetry(payload)
    state
  end

  defp handle_bridge_event(%{"type" => "WatcherFileIndexed", "payload" => payload}, state) do
    path = payload["path"] || "unknown"
    status = bridge_file_status(payload["status"] || "unknown")

    Axon.Watcher.Telemetry.report_finish("bridge:#{path}", path, status)
    Phoenix.PubSub.local_broadcast(
      AxonDashboard.PubSub,
      bridge_topic(),
      {:file_indexed, path, status}
    )
    state
  end

  defp handle_bridge_event(_, state), do: state

  defp record_file_indexed(payload) do
    worker_id = "bridge:#{payload["trace_id"] || payload["path"] || "unknown"}"
    status = bridge_file_status(payload["status"] || "ok")

    if path = payload["path"] do
      Axon.Watcher.Telemetry.report_finish(worker_id, path, status)
    end

    if (payload["t0"] || 0) > 0 do
      Axon.Watcher.Tracer.record_trace(
        payload["trace_id"] || "none",
        payload["path"] || "unknown",
        payload["t0"] || 0,
        payload["t1"] || 0,
        payload["t2"] || 0,
        payload["t3"] || 0,
        payload["t4"] || 0
      )
    end
  end

  defp bridge_file_status("ok"), do: :ok
  defp bridge_file_status("indexed_degraded"), do: :degraded
  defp bridge_file_status(_), do: :error
end
