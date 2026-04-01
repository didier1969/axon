# Copyright (c) Didier Stadelmann. All rights reserved.
defmodule Axon.Watcher.PoolProtocolTest do
  use ExUnit.Case, async: true

  alias Axon.Watcher.PoolProtocol

  test "split_lines/1 separates complete lines and keeps tail buffer" do
    assert PoolProtocol.split_lines("a\nb\n") == {["a", "b"], ""}
    assert PoolProtocol.split_lines("a\nb") == {["a"], "b"}
  end
end
