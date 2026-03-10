defmodule Axon.ScannerIntegrationTest do
  use ExUnit.Case

  test "scan/1 returns a list of files including mix.exs" do
    files = Axon.Scanner.scan(".")
    assert is_list(files)
    assert Enum.any?(files, fn file -> String.ends_with?(file, "mix.exs") end)
  end
end
