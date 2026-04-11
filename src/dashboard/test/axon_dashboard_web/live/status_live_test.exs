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
    assert html =~ "Vector Ready File (Derived)"
    assert html =~ "Vector Ready File Flag"
    assert html =~ "Chunk Embeddings"
    assert html =~ "Graph Embeddings"
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
           "db_file_bytes" => 805_306_368,
           "db_wal_bytes" => 134_217_728,
           "db_total_bytes" => 939_524_096,
           "duckdb_memory_bytes" => 402_653_184,
           "ingress_enabled" => true,
           "ingress_buffered_entries" => 42,
           "ingress_subtree_hints" => 3,
           "ingress_flush_count" => 9,
           "ingress_last_promoted_count" => 18
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
end
