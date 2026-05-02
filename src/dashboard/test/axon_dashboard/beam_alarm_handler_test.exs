# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboard.BeamAlarmHandlerTest do
  @moduledoc """
  REQ-AXO-094 — verifies the gen_event handler converts the BEAM
  alarm shapes (atom-only, {alarm_id, data}, charlist disk path)
  into the canonical `BEAM_ALARM <json>` line with the right
  `alarm` and `action` fields. Cross-process delivery to the brain
  socket is covered by the Rust receiver tests in
  `main_telemetry_beam_alarm_tests.rs`; here we pin only the
  encoding contract on the Elixir side.
  """

  use ExUnit.Case, async: false

  alias AxonDashboard.BeamAlarmHandler

  setup do
    # Capture forwarded commands by replacing BridgeClient with a
    # spy via the message-passing GenServer cast pattern. We cannot
    # mock easily, so we put the test process as the named server
    # for the duration of the test.
    bridge_pid = Process.whereis(AxonDashboard.BridgeClient)
    if bridge_pid do
      :ok = GenServer.stop(AxonDashboard.BridgeClient, :normal, 1000)
    end
    test_pid = self()
    {:ok, spy} =
      GenServer.start_link(
        AxonDashboardTest.BridgeSpy,
        test_pid,
        name: AxonDashboard.BridgeClient
      )

    on_exit(fn ->
      if Process.alive?(spy), do: GenServer.stop(spy, :normal, 1000)
    end)

    {:ok, %{spy: spy}}
  end

  test "set_alarm with {alarm_id, data} forwards BEAM_ALARM set" do
    BeamAlarmHandler.forward(:system_memory_high_watermark, "set")
    assert_receive {:bridge_command, "BEAM_ALARM " <> json}, 500
    assert {:ok, decoded} = Jason.decode(json)
    assert decoded["alarm"] == "system_memory_high_watermark"
    assert decoded["action"] == "set"
  end

  test "clear forwards BEAM_ALARM clear" do
    BeamAlarmHandler.forward(:system_memory_high_watermark, "clear")
    assert_receive {:bridge_command, "BEAM_ALARM " <> json}, 500
    assert {:ok, %{"action" => "clear"}} = Jason.decode(json)
  end

  test "charlist disk path serializes as binary" do
    BeamAlarmHandler.forward(~c"/", "set")
    assert_receive {:bridge_command, "BEAM_ALARM " <> json}, 500
    assert {:ok, %{"alarm" => "/"}} = Jason.decode(json)
  end

  test "handle_event {:set_alarm, atom_id} relays to BridgeClient" do
    {:ok, state} = BeamAlarmHandler.init([])
    {:ok, _new_state} =
      BeamAlarmHandler.handle_event({:set_alarm, :disk_almost_full}, state)
    assert_receive {:bridge_command, "BEAM_ALARM " <> json}, 500
    assert {:ok, %{"alarm" => "disk_almost_full", "action" => "set"}} = Jason.decode(json)
  end

  test "handle_event {:clear_alarm, {atom, data}} relays clear" do
    {:ok, state} = BeamAlarmHandler.init([])
    {:ok, _new_state} =
      BeamAlarmHandler.handle_event(
        {:clear_alarm, {:system_memory_high_watermark, []}},
        state
      )
    assert_receive {:bridge_command, "BEAM_ALARM " <> json}, 500
    assert {:ok, %{"action" => "clear"}} = Jason.decode(json)
  end
end

defmodule AxonDashboardTest.BridgeSpy do
  @moduledoc """
  Test-only stand-in for AxonDashboard.BridgeClient. Forwards every
  `send_command/1` cast as `{:bridge_command, line}` to the test
  process so assertions can pin the encoded payload.
  """
  use GenServer

  @impl true
  def init(test_pid), do: {:ok, test_pid}

  @impl true
  def handle_cast({:send_command, line}, test_pid) do
    send(test_pid, {:bridge_command, line})
    {:noreply, test_pid}
  end

  def handle_cast(_, state), do: {:noreply, state}

  @impl true
  def handle_call(_, _from, state), do: {:reply, :ok, state}
end
