# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboard.BridgeClient do
  use GenServer
  require Logger

  @socket_path "/tmp/axon-telemetry.sock"

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def get_state do
    GenServer.call(__MODULE__, :get_state)
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
       engine_state: :idle
     }}
  end

  def handle_call(:get_state, _from, state) do
    {:reply, state, state}
  end

  def handle_cast(_message, state), do: {:noreply, state}

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
    lines = String.split(data, "\n", trim: true)

    new_state =
      Enum.reduce(lines, state, fn line, acc ->
        if not String.contains?(line, "Axon Bridge Ready") do
          case Jason.decode(line) do
            {:ok, event} ->
              acc = handle_bridge_event(event, acc)

              Phoenix.PubSub.broadcast(
                AxonDashboard.PubSub,
                "bridge_events",
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
    send(self(), :connect)
    {:noreply, %{state | socket: nil, engine_state: :idle}}
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

        Phoenix.PubSub.broadcast(
          AxonDashboard.PubSub,
          "bridge_events",
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
    status_str = payload["status"] || "unknown"
    status = if status_str == "ok", do: :ok, else: :error

    Axon.Watcher.Telemetry.report_finish("bridge:#{path}", path, status)
    Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:file_indexed, path, status})
    state
  end

  defp handle_bridge_event(_, state), do: state

  defp record_file_indexed(payload) do
    worker_id = "bridge:#{payload["trace_id"] || payload["path"] || "unknown"}"
    status = if payload["status"] == "ok", do: :ok, else: :error

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
end
