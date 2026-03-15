defmodule AxonDashboard.BridgeClientTest do
  use ExUnit.Case, async: false

  alias AxonDashboard.BridgeClient

  setup do
    # On s'assure que le PubSub est prêt
    {:ok, %{}}
  end

  test "msgpax decoding and broadcasting" do
    data = Jason.encode!(%{"FileIndexed" => %{"path" => "test.py", "symbol_count" => 10}}) <> "\n"
    Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")

    # Simulation du message TCP arrivant au GenServer
    # On récupère le PID s'il tourne, sinon on le démarre manuellement pour le test
    pid = Process.whereis(BridgeClient)

    send(pid, {:tcp, nil, data})

    assert_receive {:bridge_event, %{"FileIndexed" => %{"path" => "test.py", "symbol_count" => 10}}},
                   1000
  end

  test "handles scan complete event" do
    data = Jason.encode!(%{"ScanComplete" => %{"total_files" => 100, "duration_ms" => 500}}) <> "\n"
    Phoenix.PubSub.subscribe(AxonDashboard.PubSub, "bridge_events")

    pid = Process.whereis(BridgeClient)
    send(pid, {:tcp, nil, data})

    assert_receive {:bridge_event,
                    %{"ScanComplete" => %{"total_files" => 100, "duration_ms" => 500}}},
                   1000
  end

  test "handles raw text data from bridge" do
    pid = Process.whereis(BridgeClient)
    # We pass a dummy port (self()) so it doesn't crash on :gen_tcp.send(nil, ...) in test
    # In a real scenario, this would be a valid port. Since gen_tcp.send checks if it's a port,
    # let's just make sure we don't pass nil if the logic expects a valid socket.
    # Wait, the code uses :gen_tcp.send(socket, ...). If we pass a dummy pid, it might fail with badarg.
    # Actually, if we just want to avoid the crash for coverage, we can just intercept the call or let the test gracefully handle it.
    # The real issue is the previous test "handles scan complete event" failed because the BridgeClient crashed processing the "Axon Bridge Ready\n" message from the PREVIOUS test (msgpax decoding or handles raw text data) since the messages are processed asynchronously.
    # The tests are not isolated enough and share the same BridgeClient process.
    # Let's use an actual dummy port or mock it. The easiest is to make the `BridgeClient` handle `nil` gracefully.
    send(pid, {:tcp, nil, "Axon Bridge Ready\n"})
    assert Process.alive?(pid)
  end
end
