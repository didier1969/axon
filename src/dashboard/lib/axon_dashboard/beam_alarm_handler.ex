# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboard.BeamAlarmHandler do
  @moduledoc """
  REQ-AXO-094 — gen_event handler attached to Erlang/OTP's
  `:alarm_handler` event manager. When SASL fires
  `:set_alarm` or `:clear_alarm` (e.g. `:system_memory_high_watermark`,
  `{:disk_almost_full, "/"}`) we forward a structured
  `BEAM_ALARM <json>\\n` line up the existing telemetry socket via
  `AxonDashboard.BridgeClient.send_command/1`.

  The brain owns the alarm→subsystem mapping (see
  `main_telemetry::handle_beam_alarm` and `beam_alarm_to_subsystem`)
  so this handler only relays raw observations. Unknown alarms are
  forwarded too — the brain decides whether to act or ignore.

  Per DEC-AXO-063 user pick A: dashboard pushes (vs brain pull). The
  push pattern co-locates with the existing telemetry socket, keeps
  the brain's status() pull-only, and provides ≈0 latency for alarm
  propagation.
  """

  @behaviour :gen_event

  require Logger

  @impl true
  def init(_args), do: {:ok, %{}}

  @impl true
  def handle_event({:set_alarm, {alarm_id, _data}}, state) do
    forward(alarm_id, "set")
    {:ok, state}
  end

  def handle_event({:set_alarm, alarm_id}, state) when is_atom(alarm_id) do
    forward(alarm_id, "set")
    {:ok, state}
  end

  def handle_event({:clear_alarm, {alarm_id, _data}}, state) do
    forward(alarm_id, "clear")
    {:ok, state}
  end

  def handle_event({:clear_alarm, alarm_id}, state) do
    forward(alarm_id, "clear")
    {:ok, state}
  end

  def handle_event(_other, state), do: {:ok, state}

  @impl true
  def handle_call(_request, state), do: {:ok, :ok, state}

  @impl true
  def handle_info(_info, state), do: {:ok, state}

  @impl true
  def code_change(_old_vsn, state, _extra), do: {:ok, state}

  @impl true
  def terminate(_args, _state), do: :ok

  # REQ-AXO-094 — convert the BEAM alarm id (atom or charlist for
  # disk path) to a stable string and push the JSON-line command
  # through BridgeClient. We do not couple to the brain's mapping
  # table here; whatever name SASL fires gets forwarded. The brain
  # silently ignores alarms outside the canonical set.
  @doc false
  def forward(alarm_id, action) when action in ["set", "clear"] do
    name = stringify_alarm_id(alarm_id)
    payload = %{"alarm" => name, "action" => action}

    case Jason.encode(payload) do
      {:ok, json} ->
        AxonDashboard.BridgeClient.send_command("BEAM_ALARM " <> json)

      {:error, reason} ->
        Logger.warning(
          "[BEAM_ALARM] failed to encode payload for #{inspect(alarm_id)}: #{inspect(reason)}"
        )
    end
  end

  defp stringify_alarm_id(id) when is_atom(id), do: Atom.to_string(id)
  defp stringify_alarm_id(id) when is_binary(id), do: id
  defp stringify_alarm_id(id) when is_list(id), do: List.to_string(id)
  defp stringify_alarm_id(id), do: inspect(id)
end
