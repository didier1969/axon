defmodule Axon.ResourceMonitorTest do
  use ExUnit.Case, async: false

  alias Axon.ResourceMonitor

  setup do
    # The application starts Axon.ResourceMonitor automatically since we added it to
    # the supervision tree. We can just test the public API.
    :ok
  end

  test "get_system_load/0 returns a map with :cpu and :ram percentages" do
    load = ResourceMonitor.get_system_load()

    assert is_map(load)
    assert Map.has_key?(load, :cpu)
    assert Map.has_key?(load, :ram)

    assert is_number(load.cpu)
    assert load.cpu >= 0.0

    assert is_number(load.ram)
    assert load.ram >= 0.0 and load.ram <= 100.0
  end

  test "handle_info(:poll, state) updates the state" do
    # Test the callback in isolation to avoid spawning a duplicate polling loop
    {:noreply, load} = ResourceMonitor.handle_info(:poll, %{})

    assert is_map(load)
    assert Map.has_key?(load, :cpu)
    assert Map.has_key?(load, :ram)
  end
end
