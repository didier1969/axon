defmodule AxonDashboardWeb.StatusLiveTest do
  use AxonDashboardWeb.ConnCase
  import Phoenix.LiveViewTest

  test "renders waiting status initially", %{conn: conn} do
    {:ok, _view, html} = live(conn, "/")
    assert html =~ "Waiting"
    assert html =~ "Total Symbols"
  end

  test "updates stats on bridge event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")
    
    # Simuler l'arrivée d'un événement via le PubSub
    event = ["FileIndexed", %{"path" => "lib/core.ex", "symbol_count" => 42}]
    Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})

    # Vérifier que l'UI s'est mise à jour
    assert render(view) =~ "Processing"
    assert render(view) =~ "42"
    assert render(view) =~ "lib/core.ex"
  end

  test "completes on scan complete event", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")
    
    event = ["ScanComplete", %{"total_files" => 10, "duration_ms" => 100}]
    Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:bridge_event, event})

    assert render(view) =~ "Complete"
  end
end
