# Copyright (c) Didier Stadelmann. All rights reserved.
defmodule Axon.Watcher.PoolProtocol do
  @moduledoc false

  def split_lines(data) do
    if String.ends_with?(data, "\n") do
      {String.split(data, "\n", trim: true), ""}
    else
      parts = String.split(data, "\n")
      {Enum.slice(parts, 0..-2//1), List.last(parts)}
    end
  end
end
