defmodule LiveView.Witness do
  @moduledoc """
  The Elixir Contract API for LiveView.Witness.
  """

  @doc """
  Pushes a rendering contract to the client.

  It generates a unique ID for the contract, registers the current process
  in the Registry under that ID, and pushes the "phx-witness:contract" event.

  Returns `{:ok, id, socket}`.

  ## Examples

      iex> {:ok, id, socket} = LiveView.Witness.expect_ui(socket, ".project-grid")
      socket

  """
  @spec expect_ui(Phoenix.LiveView.Socket.t() | Phoenix.LiveViewTest.View.t(), String.t(), keyword() | map()) ::
          {:ok, String.t(), any()}
  def expect_ui(socket_or_view, selector, expectations \\ [])

  def expect_ui(%Phoenix.LiveView.Socket{} = socket, selector, expectations) do
    id = :crypto.strong_rand_bytes(8) |> Base.encode16()

    # Register the current process for this expectation id
    {:ok, _} = Registry.register(LiveView.Witness.Registry, id, :ok)

    contract = %{
      id: id,
      selector: selector,
      expectations: Map.new(expectations)
    }

    socket = Phoenix.LiveView.push_event(socket, "phx-witness:contract", contract)
    {:ok, id, socket}
  end

  def expect_ui(%Phoenix.LiveViewTest.View{} = view, selector, _expectations) do
    id = :crypto.strong_rand_bytes(8) |> Base.encode16()

    # Register the current process for this expectation id
    {:ok, _} = Registry.register(LiveView.Witness.Registry, id, :ok)

    # In a test view, we can't easily push an event to the "client",
    # but we register the expectation so verify_ui! can wait for it.
    {:ok, id, view}
  end

  @doc """
  Reports a certificate received from the client.
  """
  @spec report_certificate(map()) :: :ok
  def report_certificate(report) do
    id = Map.fetch!(report, "id")

    Registry.dispatch(LiveView.Witness.Registry, id, fn entries ->
      for {pid, _} <- entries, do: send(pid, {:witness_report, report})
    end)
  end

  @doc """
  Synchronously waits for a report from the client and verifies it.

  Raises an error if verification fails or times out.
  """
  @spec verify_ui!(String.t(), timeout()) :: :ok | no_return()
  def verify_ui!(id, timeout \\ 5000) do
    receive do
      {:witness_report, %{"id" => ^id, "status" => "ok"}} ->
        Registry.unregister(LiveView.Witness.Registry, id)
        :ok

      {:witness_report, %{"id" => ^id, "status" => "error", "message" => msg}} ->
        Registry.unregister(LiveView.Witness.Registry, id)
        raise "LiveView.Witness verification failed for #{id}: #{msg}"

      {:witness_report, %{"id" => ^id, "status" => "error"}} ->
        Registry.unregister(LiveView.Witness.Registry, id)
        raise "LiveView.Witness verification failed for #{id}"
    after
      timeout ->
        Registry.unregister(LiveView.Witness.Registry, id)
        raise "LiveView.Witness verification timeout for #{id}"
    end
  end
end
