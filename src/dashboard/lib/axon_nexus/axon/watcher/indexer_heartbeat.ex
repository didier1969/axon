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
  @default_poll_ms 1_000

  ## Public API

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Return the latest snapshot or `nil` if the GenServer is not started yet.

  `catch :exit, _ -> nil` is intentionally retained : during a brain restart
  or supervisor swap the GenServer process may briefly be gone between the
  `whereis` lookup and the actual `call`. Returning `nil` (vs propagating
  exit) is the right defensive default for LiveView mount callsites that
  should not crash on transient unavailability.
  """
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
      last_tick_ms: nil,
      # REQ-AXO-901803 cat C15 — cache mtime to skip File.read when unchanged
      last_mtime: nil
    }

    # REQ-AXO-901803 cat C14 — `:timer.send_interval` over `Process.send_after`
    # recurrent : ticks are skipped if `handle_info` is busy, preventing queue
    # accumulation under FS-slow conditions. Initial poll triggered immediately
    # via `send(self(), :tick)` so the first snapshot lands without waiting
    # for the first interval boundary.
    {:ok, _ref} = :timer.send_interval(poll_interval_ms(), self(), :tick)
    send(self(), :tick)

    {:ok, state}
  end

  @impl true
  def handle_info(:tick, state) do
    state = poll(state)
    # No more Process.send_after — :timer.send_interval drives the cadence.
    {:noreply, state}
  end

  @impl true
  def handle_call(:latest, _from, state) do
    {:reply, state.latest, state}
  end

  ## Internals

  # REQ-AXO-901802 + REQ-AXO-901803 (MIL-AXO-028 cat B/C) — single source
  # via Application.env populated by config/runtime.exs. No more
  # File.cwd!() at module load + walking parent directories.
  defp default_path do
    Application.get_env(:axon_dashboard, __MODULE__, [])
    |> Keyword.get(:path) ||
      raise """
      Axon.Watcher.IndexerHeartbeat: no `:path` configured.
      Set via config/runtime.exs (driven by AXON_INSTANCE_KIND) or override
      AXON_INDEXER_HEARTBEAT_PATH explicitly.
      """
  end

  # REQ-AXO-901803 cat C11 — poll cadence read from Application.env at init.
  # Not a `@module_attr` so tests can override per-suite without recompile.
  defp poll_interval_ms do
    Application.get_env(:axon_dashboard, __MODULE__, [])
    |> Keyword.get(:poll_ms, @default_poll_ms)
  end

  defp poll(state) do
    now = System.monotonic_time(:millisecond)

    # REQ-AXO-901803 cat C15 — cache mtime; skip File.read when unchanged.
    # File.stat is ~100× cheaper than File.read+Jason.decode on a multi-KB
    # heartbeat JSON, and the file changes ~1×/second normally. Most ticks
    # become a single stat() call + broadcast of cached snapshot.
    case File.stat(state.path, time: :posix) do
      {:ok, %File.Stat{mtime: mtime}} when mtime == state.last_mtime and not is_nil(state.latest) ->
        # File unchanged since last read AND we have a cached snapshot.
        # Re-broadcast the cached snapshot so newly-mounted LiveViews still
        # receive an event without waiting for the next mtime change.
        broadcast(cached_event(state.latest))
        %{state | last_tick_ms: now}

      {:ok, %File.Stat{mtime: mtime}} ->
        # File changed (or first read after start) — read + parse + broadcast.
        state |> read_and_broadcast(now) |> Map.put(:last_mtime, mtime)

      {:error, reason} ->
        broadcast({:indexer_heartbeat_missing, %{reason: reason, path: state.path}})
        %{state | latest: %{status: :missing, reason: reason, path: state.path}, last_tick_ms: now}
    end
  end

  defp cached_event(%{stale: true} = snap), do: {:indexer_heartbeat_stale, snap}
  defp cached_event(%{status: :ok} = snap), do: {:indexer_heartbeat, snap}
  defp cached_event(snap), do: {:indexer_heartbeat_missing, %{reason: :unknown_shape, snapshot: snap}}

  defp read_and_broadcast(state, now) do
    case File.read(state.path) do
      {:ok, raw} ->
        case Jason.decode(raw) do
          {:ok, json} ->
            {snap, new_total} = build_snapshot(json, state, now)
            broadcast(cached_event(snap))

            %{
              state
              | latest: snap,
                last_chunks_embedded_total: new_total,
                last_tick_ms: now
            }

          {:error, reason} ->
            Logger.debug("IndexerHeartbeat parse error: #{inspect(reason)}")
            broadcast({:indexer_heartbeat_missing, %{reason: :parse_error}})
            %{state | latest: %{status: :error, reason: :parse_error}, last_tick_ms: now}
        end

      {:error, reason} ->
        broadcast({:indexer_heartbeat_missing, %{reason: reason, path: state.path}})
        %{state | latest: %{status: :missing, reason: reason, path: state.path}, last_tick_ms: now}
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
