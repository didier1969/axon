defmodule LiveView.Witness.EngineTest do
  use ExUnit.Case, async: true

  test "synchronous verification flow" do
    # Simulate a LiveView socket
    socket = %Phoenix.LiveView.Socket{}

    # 1. Start an expectation
    {:ok, id, _socket} = LiveView.Witness.expect_ui(socket, ".my-selector")

    # 2. Simulate client report (in a separate task to simulate asynchronous behavior)
    Task.start(fn ->
      Process.sleep(100)
      LiveView.Witness.report_certificate(%{"id" => id, "status" => "ok"})
    end)

    # 3. Synchronously verify
    assert :ok == LiveView.Witness.verify_ui!(id)
  end

  test "failed verification flow" do
    {:ok, id, _socket} = LiveView.Witness.expect_ui(%Phoenix.LiveView.Socket{}, ".my-selector")

    Task.start(fn ->
      Process.sleep(100)
      LiveView.Witness.report_certificate(%{"id" => id, "status" => "error", "message" => "Element not found"})
    end)

    assert_raise RuntimeError, ~r/LiveView.Witness verification failed for #{id}: Element not found/, fn ->
      LiveView.Witness.verify_ui!(id)
    end
  end

  test "timeout verification flow" do
    {:ok, id, _socket} = LiveView.Witness.expect_ui(%Phoenix.LiveView.Socket{}, ".my-selector")

    assert_raise RuntimeError, ~r/LiveView.Witness verification timeout for #{id}/, fn ->
      LiveView.Witness.verify_ui!(id, 50)
    end
  end
end
