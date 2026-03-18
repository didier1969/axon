defmodule LiveView.Witness.Token do
  @moduledoc """
  A simple Agent to store a dynamic session token for the Oracle endpoint.
  """
  use Agent

  @doc """
  Starts the token agent.
  """
  def start_link(_opts) do
    token = :crypto.strong_rand_bytes(16) |> Base.encode16(case: :lower)
    Agent.start_link(fn -> token end, name: __MODULE__)
  end

  @doc """
  Gets the current token.
  """
  def get do
    Agent.get(__MODULE__, & &1)
  end

  @doc """
  Verifies if the given token matches the stored token.
  """
  def verify?(token) do
    get() == token
  end
end
