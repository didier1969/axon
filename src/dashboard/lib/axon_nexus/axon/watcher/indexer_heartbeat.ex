defmodule Axon.Watcher.IndexerHeartbeat do
  @moduledoc """
  Polls the indexer's on-disk runtime heartbeat at
  `.axon/run-indexer/runtime-heartbeat.json` (resolved relative to the
  axon repo root, override via `AXON_INDEXER_HEARTBEAT_PATH`).

  Broadcasts to PubSub topic `bridge_events` on every poll, allowing
  any LiveView to subscribe and stay current without touching the FS:

      {:indexer_heartbeat, snapshot :: map}    # ok, fresh
      {:indexer_heartbeat_stale, %{age_s, ...}} # file too old
      {:indexer_heartbeat_missing, %{reason}}   # file missing / unreadable

  This module also computes derived deltas (chunks_embedded delta per
  tick) so that LiveViews don't need to remember previous values.

  Heartbeat shape (observed on live 2026-05-21):
    %{
      "build_id" => "v0.8.0-...",
      "degraded_reason" => "embedder_provider_fallback: ...",
      "embedder_provider" => %{...},   # REQ-AXO-91572 : unreliable
      "process_role" => "indexer",
      "runtime_mode" => "indexer_full",
      "runtime_telemetry" => %{
        "chunk_embeddings_per_second" => 0.0,
        "chunk_embeddings_rate_window_ms" => 5000,
        "graph_workers_active_current" => 11,
        "graph_workers_started_total" => 11,
        "ingress_buffered_entries" => 14232,
        "vector_chunks_embedded_total" => 0,
        "file_vectorization_queue" => %{"inflight" => 0, "queued" => 0, "total" => 0},
        "ingress_subtree_hint_*" => ...
      },
      "stale" => false, "stale_after_ms" => 5000
    }

  Per-stage A1/A2/A3/B1/B2/B3 counters are NOT in the heartbeat ; the
  brain's `embedding_status` MCP tool exposes the configured worker
  counts. The Pipeline LiveView merges both sources.
  """

  use GenServer

  require Logger

  @topic "bridge_events"
  @poll_ms 1_000

  ## Public API

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc "Return the latest snapshot (or `nil` if not yet polled)."
  def latest do
    case GenServer.whereis(__MODULE__) do
      nil -> nil
      _pid -> safe_call(:latest)
    end
  end

  defp safe_call(msg) do
    GenServer.call(__MODULE__, msg, 800)
  catch
    :exit, _ -> nil
  end

  ## GenServer

  @impl true
  def init(opts) do
    state = %{
      path: Keyword.get(opts, :path, default_path()),
      latest: nil,
      last_chunks_embedded_total: nil,
      last_tick_ms: nil
    }

    Process.send_after(self(), :tick, 200)
    {:ok, state}
  end

  @impl true
  def handle_info(:tick, state) do
    state = poll(state)
    Process.send_after(self(), :tick, @poll_ms)
    {:noreply, state}
  end

  @impl true
  def handle_call(:latest, _from, state) do
    {:reply, state.latest, state}
  end

  ## Internals

  defp default_path do
    System.get_env("AXON_INDEXER_HEARTBEAT_PATH") ||
      candidates()
      |> Enum.find(&File.exists?/1)
      |> Kernel.||(List.first(candidates()))
  end

  defp candidates do
    cwd = File.cwd!()

    [
      Path.join([cwd, ".axon", "run-indexer", "runtime-heartbeat.json"]),
      Path.expand("../.axon/run-indexer/runtime-heartbeat.json", cwd),
      Path.expand("../../.axon/run-indexer/runtime-heartbeat.json", cwd),
      Path.expand("../../../.axon/run-indexer/runtime-heartbeat.json", cwd)
    ]
  end

  defp poll(state) do
    now = System.monotonic_time(:millisecond)

    case File.read(state.path) do
      {:ok, raw} ->
        case Jason.decode(raw) do
          {:ok, json} ->
            {snap, new_total} = build_snapshot(json, state, now)

            payload =
              cond do
                snap.stale == true ->
                  {:indexer_heartbeat_stale, snap}

                snap.status == :ok ->
                  {:indexer_heartbeat, snap}

                true ->
                  {:indexer_heartbeat_missing, %{reason: :unknown_shape, snapshot: snap}}
              end

            broadcast(payload)

            %{
              state
              | latest: snap,
                last_chunks_embedded_total: new_total,
                last_tick_ms: now
            }

          {:error, reason} ->
            Logger.debug("IndexerHeartbeat parse error: #{inspect(reason)}")
            broadcast({:indexer_heartbeat_missing, %{reason: :parse_error}})
            %{state | latest: %{status: :error, reason: :parse_error}}
        end

      {:error, reason} ->
        broadcast({:indexer_heartbeat_missing, %{reason: reason, path: state.path}})
        %{state | latest: %{status: :missing, reason: reason, path: state.path}}
    end
  end

  defp build_snapshot(json, state, now_ms) do
    rt = Map.get(json, "runtime_telemetry", %{})

    chunks_embedded_total = num(rt, "vector_chunks_embedded_total")
    chunks_per_second_window = num(rt, "chunk_embeddings_per_second")

    elapsed_s =
      case state.last_tick_ms do
        nil -> 1.0
        prev -> max(0.001, (now_ms - prev) / 1_000)
      end

    delta_chunks =
      case state.last_chunks_embedded_total do
        nil -> 0.0
        prev -> max(0, chunks_embedded_total - prev)
      end

    chunks_per_second_observed = delta_chunks / elapsed_s

    snap = %{
      status: :ok,
      received_at_ms: now_ms,
      wall_ms: System.system_time(:millisecond),
      path: state.path,
      stale: Map.get(json, "stale", false),
      stale_after_ms: Map.get(json, "stale_after_ms", 5000),
      observed_age_ms: Map.get(json, "observed_age_ms"),
      build_id: Map.get(json, "build_id"),
      release_version: Map.get(json, "release_version"),
      runtime_mode: Map.get(json, "runtime_mode"),
      process_role: Map.get(json, "process_role"),
      runtime_identity: Map.get(json, "runtime_identity"),
      degraded_reason: Map.get(json, "degraded_reason"),
      embedder_provider: %{
        requested: get_in(json, ["embedder_provider", "requested"]),
        effective: get_in(json, ["embedder_provider", "effective"]),
        init_error: get_in(json, ["embedder_provider", "init_error"])
      },
      telemetry: %{
        chunk_embeddings_per_second: chunks_per_second_window,
        chunk_embeddings_per_second_observed: chunks_per_second_observed,
        chunk_embeddings_rate_window_ms: num(rt, "chunk_embeddings_rate_window_ms"),
        vector_chunks_embedded_total: chunks_embedded_total,
        vector_chunks_embedded_delta: delta_chunks,
        graph_workers_active_current: num(rt, "graph_workers_active_current"),
        graph_workers_started_total: num(rt, "graph_workers_started_total"),
        ingress_buffered_entries: num(rt, "ingress_buffered_entries"),
        ingress_hot_entries: num(rt, "ingress_hot_entries"),
        ingress_scan_entries: num(rt, "ingress_scan_entries"),
        ingress_subtree_hint_in_flight: num(rt, "ingress_subtree_hint_in_flight"),
        ingress_subtree_hint_accepted_total: num(rt, "ingress_subtree_hint_accepted_total"),
        ingress_subtree_hint_blocked_total: num(rt, "ingress_subtree_hint_blocked_total"),
        ingress_subtree_hint_suppressed_total: num(rt, "ingress_subtree_hint_suppressed_total"),
        file_vectorization_queue_total:
          get_in(rt, ["file_vectorization_queue", "total"]) || 0,
        file_vectorization_queue_inflight:
          get_in(rt, ["file_vectorization_queue", "inflight"]) || 0,
        file_vectorization_queue_queued:
          get_in(rt, ["file_vectorization_queue", "queued"]) || 0,
        graph_projection_queue_total:
          get_in(rt, ["graph_projection_queue", "total"]) || 0,
        graph_projection_queue_inflight:
          get_in(rt, ["graph_projection_queue", "inflight"]) || 0,
        ready_queue_chunks_current: num(rt, "ready_queue_chunks_current"),
        ready_queue_chunks_small: num(rt, "ready_queue_chunks_small"),
        ready_queue_chunks_medium: num(rt, "ready_queue_chunks_medium"),
        ready_queue_chunks_large: num(rt, "ready_queue_chunks_large"),
        utility_first_scheduler_state: Map.get(rt, "utility_first_scheduler_state"),
        utility_first_scheduler_reason: Map.get(rt, "utility_first_scheduler_reason"),
        service_pressure: Map.get(rt, "service_pressure"),
        last_consumed_batch_lane: Map.get(rt, "last_consumed_batch_lane"),
        homogeneous_batches_total: num(rt, "homogeneous_batches_total"),
        mixed_fallback_batches_total: num(rt, "mixed_fallback_batches_total")
      }
    }

    {snap, chunks_embedded_total}
  end

  defp num(map, key) do
    case Map.get(map, key) do
      n when is_number(n) -> n
      _ -> 0
    end
  end

  defp broadcast(payload) do
    Phoenix.PubSub.broadcast(AxonDashboard.PubSub, @topic, payload)
  end
end
