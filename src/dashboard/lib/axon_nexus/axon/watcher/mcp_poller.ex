defmodule Axon.Watcher.McpPoller do
  @moduledoc """
  Periodically calls `embedding_status` against the MCP server and
  broadcasts the canonical pipeline_a/pipeline_b shape over PubSub.

  This is the second half of the pipeline-cockpit data flow:

    IndexerHeartbeat -> runtime_telemetry (rates, queues, ingress)
    McpPoller        -> embedding_status (workers/batch config, coverage)

  Broadcasts on topic `bridge_events`:
    {:mcp_embedding_status, snapshot :: map}
    {:mcp_embedding_status_error, reason}

  Also caches latest snapshot for hot-mount LiveViews that connect
  between polls.
  """

  use GenServer

  require Logger

  alias Axon.Watcher.McpClient

  @topic "bridge_events"
  @poll_ms 3_000

  ## Public API

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc "Latest cached `embedding_status` snapshot (or nil)."
  def latest do
    case GenServer.whereis(__MODULE__) do
      nil -> nil
      _pid -> GenServer.call(__MODULE__, :latest, 800)
    end
  catch
    :exit, _ -> nil
  end

  @doc "Force a re-poll (returns immediately, broadcasts asynchronously)."
  def refresh do
    case GenServer.whereis(__MODULE__) do
      nil -> :ignore
      _pid -> GenServer.cast(__MODULE__, :refresh)
    end
  end

  ## GenServer

  @impl true
  def init(opts) do
    state = %{
      project: Keyword.get(opts, :project, "*"),
      latest: nil,
      last_error: nil
    }

    Process.send_after(self(), :tick, 1_000)
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

  @impl true
  def handle_cast(:refresh, state) do
    {:noreply, poll(state)}
  end

  ## Internals

  defp poll(state) do
    case McpClient.call_tool("embedding_status", %{"project" => state.project}) do
      {:ok, result} ->
        structured = Map.get(result, "_structured") || Map.get(result, "structuredContent") || %{}
        snap = normalize(structured, result)
        broadcast({:mcp_embedding_status, snap})
        %{state | latest: snap, last_error: nil}

      {:error, reason} ->
        Logger.debug("McpPoller error: #{inspect(reason)}")
        broadcast({:mcp_embedding_status_error, reason})
        %{state | last_error: reason}
    end
  end

  defp normalize(structured, _raw) do
    pa = Map.get(structured, "pipeline_a", %{})
    pb = Map.get(structured, "pipeline_b", %{})

    %{
      received_at_ms: System.monotonic_time(:millisecond),
      project: Map.get(structured, "project", "*"),
      total_chunks: num(structured, "total_chunks"),
      embedded_chunks: num(structured, "embedded_chunks"),
      pending_chunks: num(structured, "pending_chunks"),
      coverage_pct: num(structured, "coverage_pct"),
      indexed_files: num(structured, "indexed_files"),
      symbols: num(structured, "symbols"),
      edges: num(structured, "edges"),
      projects: num(structured, "projects"),
      runtime_idle: Map.get(structured, "runtime_idle", false),
      runtime_pending_count: num(structured, "runtime_pending_count"),
      lifecycle_phase: Map.get(structured, "lifecycle_phase"),
      lifecycle_source: Map.get(structured, "lifecycle_source"),
      lifecycle_heartbeat_age_ms: num(structured, "lifecycle_heartbeat_age_ms"),
      lifecycle_wake_count: num(structured, "lifecycle_wake_count"),
      lifecycle_sleep_count: num(structured, "lifecycle_sleep_count"),
      notify_channel: Map.get(structured, "notify_channel"),
      coldstart_poll_interval_secs: num(structured, "coldstart_poll_interval_secs"),
      pipeline_a: %{
        a1_workers: num(pa, "a1"),
        a2_workers: num(pa, "a2"),
        a3_workers: num(pa, "a3"),
        a3_batch_size: num(pa, "a3_batch_size"),
        a3_batch_timeout_ms: num(pa, "a3_batch_timeout_ms")
      },
      pipeline_b: %{
        b1_workers: num(pb, "b1"),
        b2_workers: num(pb, "b2"),
        b3_workers: num(pb, "b3"),
        b2_batch_size: num(pb, "b2_batch_size"),
        b2_batch_timeout_ms: num(pb, "b2_batch_timeout_ms"),
        b3_batch_size: num(pb, "b3_batch_size"),
        b3_batch_timeout_ms: num(pb, "b3_batch_timeout_ms"),
        a3_to_b1_buffer_cap: num(pb, "a3_to_b1_buffer_cap"),
        coldstart_batch_size: num(pb, "coldstart_batch_size")
      }
    }
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
