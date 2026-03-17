defmodule LiveView.WitnessTest do
  use ExUnit.Case
  doctest LiveView.Witness

  test "greets the world" do
    assert LiveView.Witness.hello() == :world
  end
end
