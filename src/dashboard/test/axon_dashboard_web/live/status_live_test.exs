# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboardWeb.StatusLiveTest do
  use AxonDashboardWeb.ConnCase
  import Phoenix.LiveViewTest

  setup do
    if pid = Process.whereis(AxonDashboard.BridgeClient) do
      :sys.get_state(pid)
    end

    Axon.Watcher.Telemetry.reset!()
    :ok
  end

  test "renders operator cockpit sections without external cdn assets", %{conn: conn} do
    {:ok, _view, html} = live(conn, "/")
    assert html =~ "Axon Cockpit"
    assert html =~ "Workspace"
    assert html =~ "Backlog"
    assert html =~ "Projects"
    assert html =~ "Runtime"
    assert html =~ "Memory"
    assert html =~ "Ingress"
    assert html =~ "File lifecycle"
    assert html =~ "AST / graph readiness"
    assert html =~ "Vectorization readiness"
    assert html =~ "SOLL alignment"
    assert html =~ "Terminal Files"
    assert html =~ "Files AST Ready"
    assert html =~ "Files Vectorized (Derived)"
    assert html =~ "File.vector_ready Raw Flag"
    assert html =~ "Chunk Embeddings"
    assert html =~ "Graph Embeddings (Advanced)"
    assert html =~ "data-indexed-graph-ready"
    assert html =~ "data-indexed-graph-missing"
    assert html =~ "data-indexed-degraded-graph-ready"
    assert html =~ "data-indexed-degraded-graph-missing"
    assert html =~ "data-indexed-vector-ready"
    assert html =~ "data-indexed-vector-missing"
    assert html =~ "data-indexed-degraded-vector-ready"
    assert html =~ "data-indexed-degraded-vector-missing"
    refute html =~ "fonts.googleapis.com"
    refute html =~ "fonts.gstatic.com"
    refute html =~ "cdn.jsdelivr.net"
  end

  test "updates recent activity on file indexed bridge event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event,
       %{
         "FileIndexed" => %{
           "path" => "lib/core.ex",
           "symbol_count" => 42,
           "security_score" => 95,
           "coverage_score" => 85
         }
       }}
    )

    # Retry assertion for race condition resilience
    # small yield
    assert_receive _, 10
    assert render(view) =~ "lib/core.ex"
    assert render(view) =~ "Recent Activity"
  end

  test "renders degraded file events as controlled degradation, not error", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event,
       %{
         "FileIndexed" => %{
           "path" => "lib/degraded.ex",
           "status" => "indexed_degraded"
         }
       }}
    )

    html = render(view)
    assert html =~ "lib/degraded.ex"
    assert html =~ "DEGRADED"
  end

  test "completes on scan complete event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event, %{"ScanComplete" => %{"total_files" => 10, "duration_ms" => 100}}}
    )

    # Wait for the re-render explicitly by asserting the rendered output directly
    assert render(view) =~ "Runtime reported scan completion"
    assert render(view) =~ "Workspace"
  end

  test "ignores legacy enqueue telemetry because Elixir is not a control plane", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:telemetry_event, [:axon, :watcher, :batch_enqueued], %{count: 2},
       %{queue: :indexing_default}}
    )

    refute render(view) =~ "batch_enqueued"
  end

  test "renders runtime and memory telemetry from the Rust bridge", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event,
       %{
         "RuntimeTelemetry" => %{
           "telemetry_source" => "local_runtime",
           "telemetry_process_role" => "brain",
           "telemetry_freshness_state" => "fresh",
           "telemetry_observed_age_ms" => 120,
           "budget_bytes" => 1_073_741_824,
           "reserved_bytes" => 268_435_456,
           "exhaustion_ratio" => 0.25,
           "queue_depth" => 12,
           "claim_mode" => "balanced",
           "service_pressure" => "healthy",
           "oversized_refusals_total" => 5,
           "degraded_mode_entries_total" => 2,
           "rss_bytes" => 3_221_225_472,
           "rss_anon_bytes" => 2_147_483_648,
           "rss_file_bytes" => 536_870_912,
           "vector_chunks_embedded_total" => 512,
           "chunk_embeddings_per_second" => 64.4,
           "chunk_embeddings_rate_window_ms" => 5_000,
           "prepare_inflight_chunks_current" => 11,
           "ready_queue_chunks_current" => 27,
           "ready_queue_chunks_small" => 5,
           "ready_queue_chunks_medium" => 9,
           "ready_queue_chunks_large" => 13,
           "ready_batches_small" => 1,
           "ready_batches_medium" => 2,
           "ready_batches_large" => 3,
           "mixed_fallback_batches_total" => 4,
           "homogeneous_batches_total" => 22,
           "last_consumed_batch_lane" => "large",
           "active_small_max_tokens" => 96,
           "active_medium_max_tokens" => 192,
           "graph_workers_started_total" => 2,
           "graph_workers_active_current" => 2,
           "ingress_enabled" => true,
           "ingress_buffered_entries" => 42,
           "ingress_subtree_hints" => 3,
           "ingress_flush_count" => 9,
           "ingress_last_promoted_count" => 18,
           "projected_indexer_runtime" => %{
             "available" => true,
             "telemetry_source" => "indexer_peer_heartbeat",
             "process_role" => "indexer",
             "freshness_state" => "fresh",
             "observed_age_ms" => 45,
             "telemetry" => %{
               "ingress_buffered_entries" => 73,
               "ingress_last_promoted_count" => 29,
               "vector_chunks_embedded_total" => 640,
               "ready_queue_chunks_current" => 21,
               "ready_queue_chunks_small" => 3,
               "ready_queue_chunks_medium" => 7,
               "ready_queue_chunks_large" => 11,
               "mixed_fallback_batches_total" => 1,
               "homogeneous_batches_total" => 15,
               "last_consumed_batch_lane" => "medium",
               "chunk_embeddings_per_second" => 80.0,
               "chunk_embeddings_rate_window_ms" => 5_000,
               "graph_workers_started_total" => 3,
               "graph_workers_active_current" => 2
             }
           }
         }
       }}
    )

    html = render(view)
    assert html =~ "Budget Reserved"
    assert html =~ "256 MB / 1024 MB"
    assert html =~ "25.0%"
    assert html =~ "Queue Depth"
    assert html =~ "12"
    assert html =~ "Claim Mode"
    assert html =~ "BALANCED"
    assert html =~ "Runtime Source"
    assert html =~ "LOCAL_RUNTIME"
    assert html =~ "Runtime Role"
    assert html =~ "BRAIN"
    assert html =~ "Runtime Freshness"
    assert html =~ "FRESH (120 ms)"
    assert html =~ "64.4 chunks/s (5000 ms)"
    assert html =~ "512"
    assert html =~ "Ready Chunks"
    assert html =~ "27"
    assert html =~ "Prepare Chunks"
    assert html =~ "11"
    assert html =~ "Ready Lanes"
    assert html =~ "S 5 / M 9 / L 13"
    assert html =~ "Ready Batches"
    assert html =~ "S 1 / M 2 / L 3"
    assert html =~ "Batch Shape"
    assert html =~ "H 22 / Mixed 4"
    assert html =~ "Last GPU Lane"
    assert html =~ "LARGE"
    assert html =~ "Lane Thresholds"
    assert html =~ "small&lt;=96 / medium&lt;=192 / large&gt;192"
    assert html =~ "2 active / 2 started"
    assert html =~ "Oversized"
    assert html =~ "5"
    assert html =~ "Degraded"
    assert html =~ "2"
    assert html =~ "RssAnon"
    assert html =~ "2048 MB"
    assert html =~ "DuckDB Memory"
    assert html =~ "384 MB"
    assert html =~ "Buffered Entries"
    assert html =~ "42"
    assert html =~ "Indexer Runtime"
    assert html =~ "INDEXER_PEER_HEARTBEAT"
    assert html =~ "FRESH (45 ms)"
    assert html =~ "144"
    assert html =~ "81"
    assert html =~ "80.0 chunks/s (5000 ms)"
    assert html =~ "640"
    assert html =~ "S 3 / M 7 / L 11"
    assert html =~ "H 15 / Mixed 1"
    assert html =~ "MEDIUM"
    assert html =~ "2 active / 3 started"
  end

  test "renders host pressure telemetry from runtime telemetry only", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event,
       %{
         "RuntimeTelemetry" => %{
           "cpu_load" => 61.5,
           "ram_load" => 47.0,
           "io_wait" => 12.2,
           "host_state" => "constrained",
           "host_guidance_slots" => 2
         }
       }}
    )

    html = render(view)
    assert html =~ "Host CPU"
    assert html =~ "61.5%"
    assert html =~ "Host RAM"
    assert html =~ "47.0%"
    assert html =~ "Host IO Wait"
    assert html =~ "12.2%"
    assert html =~ "Host State"
    assert html =~ "CONSTRAINED"
    assert html =~ "Guidance Slots"
    assert html =~ "2 slots"
  end

  test "ignores local backpressure telemetry because cockpit reads Rust runtime only", %{
    conn: conn
  } do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:telemetry_event, [:axon, :backpressure, :pressure_computed], %{pressure: 0.82},
       %{cpu: 61.5, ram: 47.0, io: 12.2}}
    )

    html = render(view)
    assert html =~ "Host CPU"
    assert html =~ "0.0%"
    assert html =~ "Host State"
    assert html =~ "HEALTHY"
  end

  test "reconcile accepts a real zero snapshot after IST reset instead of preserving stale counts",
       %{conn: conn} do
    initial_routes = snapshot_routes(3, 1)
    zero_routes = snapshot_routes(0, 0)

    with_dynamic_sql_gateway(initial_routes, fn update_routes ->
      {:ok, view, html} = live(conn, "/")
      assert html =~ ~s(data-known="3")

      update_routes.(zero_routes)
      send(view.pid, :reconcile_tick)

      assert eventually(fn -> render(view) =~ ~s(data-known="0") end)
      refute render(view) =~ ~s(data-known="3")
    end)
  end

  defp snapshot_routes(known, vectorized) do
    completed = known

    flow_rows =
      if known == 0 do
        [[0, 0, 0, 0, 0, 0, 0, 0]]
      else
        [[completed, 0, 0, 0, vectorized, max(completed - vectorized, 0), 0, 0]]
      end

    default_rows =
      if known == 0 do
        []
      else
        [
          ["workspace_status", nil, "indexed", completed, nil],
          ["workspace_stage", nil, "graph_indexed", completed, nil],
          ["workspace_ready", nil, "ready", completed, vectorized],
          ["project_status", "alpha", "indexed", completed, nil],
          ["project_ready", "alpha", "ready", completed, vectorized]
        ]
      end

    [
      {:default, default_rows},
      {"FROM soll.ProjectCodeRegistry", if(known == 0, do: [], else: [["alpha", "Alpha"]])},
      {"AS indexed_graph_ready", flow_rows},
      {"SELECT COUNT(*) FROM File WHERE vector_ready = TRUE", [[vectorized]]},
      {"SELECT COUNT(*) FROM ChunkEmbedding", [[vectorized * 2]]},
      {"COUNT(DISTINCT anchor_type || ':' || anchor_id) FROM GraphEmbedding", [[vectorized]]},
      {"SELECT COUNT(*) FROM Symbol", [[known * 3]]},
      {"links_count", [[known * 4]]}
    ]
  end

  defp with_dynamic_sql_gateway(initial_routes, fun) do
    :inets.start()
    :ssl.start()
    port = random_port()
    {:ok, routes_ref} = Agent.start_link(fn -> initial_routes end)

    {:ok, listener} =
      :gen_tcp.listen(port, [:binary, packet: :raw, active: false, reuseaddr: true])

    previous = Application.get_env(:axon_dashboard, Axon.Watcher.SqlGateway, [])

    Application.put_env(
      :axon_dashboard,
      Axon.Watcher.SqlGateway,
      Keyword.put(previous, :url, "http://127.0.0.1:#{port}/sql")
    )

    parent = self()

    task =
      Task.async(fn ->
        dynamic_accept_loop(listener, routes_ref, parent)
      end)

    update_routes = fn routes -> Agent.update(routes_ref, fn _ -> routes end) end

    try do
      fun.(update_routes)
    after
      Application.put_env(:axon_dashboard, Axon.Watcher.SqlGateway, previous)
      send(task.pid, :stop)
      Task.await(task, 5_000)
      Agent.stop(routes_ref)
    end
  end

  defp dynamic_accept_loop(listener, routes_ref, parent) do
    receive do
      :stop ->
        :gen_tcp.close(listener)
        :ok
    after
      50 ->
        case :gen_tcp.accept(listener, 200) do
          {:ok, socket} ->
            {:ok, request} = :gen_tcp.recv(socket, 0, 5_000)
            routes = Agent.get(routes_ref, & &1)
            body = response_body_for_request(request, routes)

            response = [
              "HTTP/1.1 200 OK\r\n",
              "content-type: application/json\r\n",
              "content-length: #{byte_size(body)}\r\n",
              "connection: close\r\n\r\n",
              body
            ]

            :ok = :gen_tcp.send(socket, response)
            :gen_tcp.close(socket)
            dynamic_accept_loop(listener, routes_ref, parent)

          {:error, :timeout} ->
            if Process.alive?(parent) do
              dynamic_accept_loop(listener, routes_ref, parent)
            else
              :gen_tcp.close(listener)
              :ok
            end
        end
    end
  end

  defp response_body_for_request(request, routes) do
    request = IO.iodata_to_binary(request)

    query =
      case Regex.run(~r/\r\n\r\n(?<body>\{.*\})/s, request, capture: :all_names) do
        [body] ->
          case Jason.decode(body) do
            {:ok, %{"query" => query}} -> query
            _ -> ""
          end

        _ ->
          ""
      end

    normalized_query = normalize_sql(query)

    if String.contains?(normalized_query, "indexed_graph_ready") and
         String.contains?(normalized_query, "indexed_degraded_vector_missing") do
      routes
      |> Enum.find_value("[]", fn
        {"AS indexed_graph_ready", rows} -> Jason.encode!(rows)
        _ -> nil
      end)
    else
      routes
      |> Enum.reject(fn {needle, _rows} -> needle == :default end)
      |> Enum.find_value(fn
        {needle, rows} when is_binary(needle) ->
          if String.contains?(normalized_query, normalize_sql(needle)),
            do: Jason.encode!(rows),
            else: nil
      end)
      |> Kernel.||(default_route_body(routes))
    end
  end

  defp default_route_body(routes) do
    Enum.find_value(routes, "[]", fn
      {:default, rows} -> Jason.encode!(rows)
      _route -> nil
    end)
    |> Kernel.||("[]")
  end

  defp eventually(fun, attempts \\ 40)

  defp eventually(fun, attempts) when attempts > 0 do
    if fun.() do
      true
    else
      Process.sleep(25)
      eventually(fun, attempts - 1)
    end
  end

  defp eventually(_fun, 0), do: false

  defp random_port do
    45_000 + rem(:erlang.unique_integer([:positive]), 10_000)
  end

  defp normalize_sql(value) do
    value
    |> to_string()
    |> String.downcase()
    |> String.replace(~r/\s+/u, " ")
    |> String.trim()
  end
end
