defmodule LiveView.Witness do
  @moduledoc """
  The Elixir Contract API for LiveView.Witness.
  """

  @doc """
  Pushes a rendering contract to the client.

  It generates a unique ID for the contract and pushes the "phx-witness:contract" event to the socket.

  ## Examples

      iex> LiveView.Witness.expect_ui(socket, ".project-grid")
      socket

  """
  @spec expect_ui(Phoenix.LiveView.Socket.t(), String.t(), keyword() | map()) :: Phoenix.LiveView.Socket.t()
  def expect_ui(socket, selector, expectations \\ []) do
    contract = %{
      id: :crypto.strong_rand_bytes(8) |> Base.encode16(),
      selector: selector,
      expectations: Map.new(expectations)
    }

    Phoenix.LiveView.push_event(socket, "phx-witness:contract", contract)
  end
end
