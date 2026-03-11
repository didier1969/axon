defmodule AxonDashboard.BridgeClientTest do
  use ExUnit.Case, async: false
  
  alias AxonDashboard.BridgeClient

  setup do
    # On s'assure que le PubSub est prêt
    {:ok, %{}}
  end

  test "msgpax decoding and broadcasting" do
    data = Msgpax.pack!(["FileIndexed", %{"path" => "test.py", "symbol_count" => 10}])
    Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")

    # Simulation du message TCP arrivant au GenServer
    # On récupère le PID s'il tourne, sinon on le démarre manuellement pour le test
    pid = Process.whereis(BridgeClient)
    
    send(pid, {:tcp, nil, data})

    assert_receive {:bridge_event, ["FileIndexed", %{"path" => "test.py", "symbol_count" => 10}]}, 1000
  end

  test "handles scan complete event" do
    data = Msgpax.pack!(["ScanComplete", %{"total_files" => 100, "duration_ms" => 500}])
    Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")

    pid = Process.whereis(BridgeClient)
    send(pid, {:tcp, nil, data})

    assert_receive {:bridge_event, ["ScanComplete", %{"total_files" => 100, "duration_ms" => 500}]}, 1000
  end

  test "handles raw text data from bridge" do
    pid = Process.whereis(BridgeClient)
    send(pid, {:tcp, nil, "Axon Bridge Ready\n"})
    # On vérifie juste que ça ne crashe pas (couverture de la branche raw)
    assert Process.alive?(pid)
  end
end
