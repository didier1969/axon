defmodule LiveView.Witness do
  @moduledoc """
  The Elixir Contract API for LiveView.Witness.
  """

  @doc """
  Returns the configured PubSub server name.
  """
  def pubsub, do: Application.get_env(:liveview_witness, :pubsub, LiveView.Witness.PubSub)

  @doc """
  Returns the configured Registry name.
  """
  def registry, do: Application.get_env(:liveview_witness, :registry, LiveView.Witness.Registry)

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
    {:ok, _} = Registry.register(registry(), id, :ok)
    # Global routing for multi-node support
    :ok = Phoenix.PubSub.subscribe(pubsub(), "witness:cert:#{id}")

    contract = %{
      id: id,
      selector: selector,
      expectations: Map.new(expectations)
    }

    :telemetry.execute([:liveview_witness, :contract, :pushed], %{count: 1}, %{id: id, selector: selector})

    socket = Phoenix.LiveView.push_event(socket, "phx-witness:contract", contract)
    {:ok, id, socket}
  end

  def expect_ui(%Phoenix.LiveViewTest.View{} = view, selector, _expectations) do
    id = :crypto.strong_rand_bytes(8) |> Base.encode16()

    # Register the current process for this expectation id
    {:ok, _} = Registry.register(registry(), id, :ok)
    # Global routing for multi-node support
    :ok = Phoenix.PubSub.subscribe(pubsub(), "witness:cert:#{id}")

    :telemetry.execute([:liveview_witness, :contract, :pushed], %{count: 1}, %{id: id, selector: selector})

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
    status = Map.get(report, "status")

    :telemetry.execute([:liveview_witness, :certificate, :received], %{count: 1}, %{
      status: status,
      id: id,
      reason: get_in(report, ["details", "reason"])
    })

    # Global broadcast for multi-node support
    Phoenix.PubSub.broadcast(pubsub(), "witness:cert:#{id}", {:witness_report, report})
  end

  @doc """
  Synchronously waits for a report from the client and verifies it.

  Raises an error if verification fails or times out.
  """
  @spec verify_ui!(String.t(), timeout()) :: :ok | no_return()
  def verify_ui!(id, timeout \\ 5000) do
    try do
      receive do
        {:witness_report, %{"id" => ^id, "status" => "ok"}} ->
          :ok

        {:witness_report, %{"id" => ^id, "status" => "error", "message" => msg}} ->
          raise "LiveView.Witness verification failed for #{id}: #{msg}"

        {:witness_report, %{"id" => ^id, "status" => "error"}} ->
          raise "LiveView.Witness verification failed for #{id}"
      after
        timeout ->
          raise "LiveView.Witness verification timeout for #{id}"
      end
    after
      Registry.unregister(registry(), id)
      Phoenix.PubSub.unsubscribe(pubsub(), "witness:cert:#{id}")
    end
  end
end
