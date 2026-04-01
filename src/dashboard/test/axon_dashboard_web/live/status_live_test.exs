# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboardWeb.StatusLiveTest do
  use AxonDashboardWeb.ConnCase
  import Phoenix.LiveViewTest

  test "renders waiting status initially", %{conn: conn} do
    {:ok, _view, html} = live(conn, "/")
    assert html =~ "Multi-Project Visualization Plane"
  end

  test "updates stats on bridge event", %{conn: conn} do
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
  end

  test "completes on scan complete event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event, %{"ScanComplete" => %{"total_files" => 10, "duration_ms" => 100}}}
    )

    # Wait for the re-render explicitly by asserting the rendered output directly
    assert render(view) =~ "Runtime reported scan completion"
  end

  test "ignores legacy enqueue telemetry because Elixir is not a control plane", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:telemetry_event, [:axon, :watcher, :batch_enqueued], %{count: 2}, %{queue: :indexing_default}}
    )

    refute render(view) =~ "Legacy path"
  end

  test "renders runtime memory telemetry from the Rust bridge", %{conn: conn} do
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
           "oversized_refusals_total" => 5,
           "degraded_mode_entries_total" => 2
         }
       }}
    )

    html = render(view)
    assert html =~ "256 MB / 1024 MB"
    assert html =~ "25.0%"
    assert html =~ "Queue Depth"
    assert html =~ "12"
    assert html =~ "Oversized Refusals"
    assert html =~ "5"
    assert html =~ "Degraded Entries"
    assert html =~ "2"
  end

  test "renders host pressure telemetry in the cockpit", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:telemetry_event, [:axon, :backpressure, :pressure_computed], %{pressure: 0.82},
       %{cpu: 61.5, ram: 47.0, io: 12.2}}
    )

    send(
      view.pid,
      {:telemetry_event, [:axon, :backpressure, :queues_paused], %{pressure: 1.02}, %{}}
    )

    html = render(view)
    assert html =~ "HOST_CPU"
    assert html =~ "61.5%"
    assert html =~ "HOST_RAM"
    assert html =~ "47.0%"
    assert html =~ "HOST_IO_WAIT"
    assert html =~ "12.2%"
    assert html =~ "HOST_STATE"
    assert html =~ "CONSTRAINED"
  end
end
