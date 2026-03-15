defmodule AxonDashboardWeb.StatusLiveTest do
  use AxonDashboardWeb.ConnCase
  import Phoenix.LiveViewTest

  test "renders waiting status initially", %{conn: conn} do
    {:ok, _view, html} = live(conn, "/")
    assert html =~ "Fleet Connection Offline"
  end

  test "updates stats on bridge event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    # Simuler l'arrivée d'un événement via PubSub
    send(
      view.pid,
      {:bridge_event, %{"FileIndexed" => %{"path" => "lib/core.ex", "symbol_count" => 42, "security_score" => 95, "coverage_score" => 85}}}
    )

    # Vérifier que l'UI s'est mise à jour
    assert render(view) =~ "42"
    assert render(view) =~ "lib/core.ex"
  end

  test "completes on scan complete event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")

    send(
      view.pid,
      {:bridge_event, %{"ScanComplete" => %{"total_files" => 10, "duration_ms" => 100}}}
    )

    assert render(view) =~ "Fleet Ingestion Complete"
  end
end
