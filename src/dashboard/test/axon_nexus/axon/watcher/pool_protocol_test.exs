defmodule Axon.Watcher.PoolProtocolTest do
  use ExUnit.Case, async: true

  alias Axon.Watcher.PoolProtocol

  test "split_lines/1 separates complete lines and keeps tail buffer" do
    assert PoolProtocol.split_lines("a\nb\n") == {["a", "b"], ""}
    assert PoolProtocol.split_lines("a\nb") == {["a"], "b"}
  end

  test "ack_targets/2 resolves a specific batch id when present" do
    batches = %{"b1" => {:from1, 2, []}, "b2" => {:from2, 3, []}}

    assert PoolProtocol.ack_targets(batches, "b2") == [{"b2", {:from2, 3, []}}]
    assert PoolProtocol.ack_targets(batches, "missing") == []
  end

  test "ack_targets/2 falls back to the oldest visible batch when id is nil" do
    batches = %{"b1" => {:from1, 2, []}}

    assert PoolProtocol.ack_targets(batches, nil) == [{"b1", {:from1, 2, []}}]
    assert PoolProtocol.ack_targets(%{}, nil) == []
  end
end
