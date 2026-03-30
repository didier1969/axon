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

  def ack_targets(batches, nil) do
    case Map.to_list(batches) do
      [] -> []
      [first | _] -> [first]
    end
  end

  def ack_targets(batches, batch_id) do
    case Map.fetch(batches, batch_id) do
      {:ok, batch} -> [{batch_id, batch}]
      :error -> []
    end
  end
end
