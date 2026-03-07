defmodule Axon.WatcherTest do
  use ExUnit.Case, async: true

  alias Axon.Watcher.PoolFacade

  @tag :integration
  test "orchestrator can ping the python slave via the pool" do
    assert PoolFacade.ping() == %{"status" => "ok", "data" => "pong"}
  end

  @tag :integration
  test "orchestrator can request a parse from the python slave via the pool" do
    content = "def test(): return 1"
    path = "test.py"
    
    case PoolFacade.parse(path, content) do
      %{"status" => "ok", "data" => %{"symbols" => symbols}} ->
        assert Enum.any?(symbols, fn s -> s["name"] == "test" end)
      error ->
        flunk("Parsing failed with: #{inspect(error)}")
    end
  end
end
