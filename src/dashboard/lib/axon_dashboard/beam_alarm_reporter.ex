# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule AxonDashboard.BeamAlarmReporter do
  @moduledoc """
  REQ-AXO-094 — supervisor-attached process that installs
  `AxonDashboard.BeamAlarmHandler` into the SASL `:alarm_handler`
  gen_event manager at boot and re-installs it if the handler
  crashes (`:gen_event.add_sup_handler/3` semantics: when the
  handler exits, this owner process receives a `{:gen_event_EXIT,
  ...}` message and is expected to either stop or re-add).

  This module exists so the alarm handler installation has a
  predictable place in the supervision tree — without it, the
  handler would have to be installed lazily and would silently miss
  early-boot alarms.
  """

  use GenServer
  require Logger

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    case install_handler() do
      :ok ->
        {:ok, %{installed: true}}

      {:error, reason} ->
        Logger.warning(
          "[BEAM_ALARM] failed to install handler at boot: #{inspect(reason)}; will retry on next gen_event_EXIT"
        )
        {:ok, %{installed: false}}
    end
  end

  @impl true
  def handle_info({:gen_event_EXIT, AxonDashboard.BeamAlarmHandler, reason}, state) do
    Logger.warning(
      "[BEAM_ALARM] handler exited (#{inspect(reason)}); re-installing"
    )
    case install_handler() do
      :ok -> {:noreply, %{state | installed: true}}
      {:error, _} -> {:noreply, %{state | installed: false}}
    end
  end

  def handle_info(_other, state), do: {:noreply, state}

  defp install_handler do
    # add_sup_handler links the handler so we receive
    # :gen_event_EXIT messages when it dies.
    :gen_event.add_sup_handler(:alarm_handler, AxonDashboard.BeamAlarmHandler, [])
  end
end
