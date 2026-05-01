# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.TelemetryTest do
  # REQ-AXO-107 — `mark_sql_snapshot_*` functions return a transition tag
  # the caller uses to decide whether to log. Without this gate the
  # cockpit floods the tmux pane with duplicate econnrefused warnings
  # whenever the brain is not running on the same instance.
  use ExUnit.Case, async: false

  alias Axon.Watcher.Telemetry

  setup do
    case :ets.whereis(:axon_telemetry) do
      :undefined ->
        :ets.new(:axon_telemetry, [:set, :public, :named_table])

      _ ->
        :ets.delete_all_objects(:axon_telemetry)
    end

    :ok
  end

  test "first failure returns :reason_changed and subsequent identical reasons return :reason_unchanged" do
    assert Telemetry.mark_sql_snapshot_error(:econnrefused, 1) == :reason_changed
    assert Telemetry.mark_sql_snapshot_error(:econnrefused, 2) == :reason_unchanged
    assert Telemetry.mark_sql_snapshot_error(:econnrefused, 3) == :reason_unchanged
  end

  test "different failure reason returns :reason_changed" do
    assert Telemetry.mark_sql_snapshot_error(:econnrefused, 1) == :reason_changed
    assert Telemetry.mark_sql_snapshot_error(:timeout, 2) == :reason_changed
    assert Telemetry.mark_sql_snapshot_error(:timeout, 3) == :reason_unchanged
    assert Telemetry.mark_sql_snapshot_error(:econnrefused, 4) == :reason_changed
  end

  test "success after a sustained error streak returns :recovered" do
    Telemetry.mark_sql_snapshot_error(:econnrefused, 1)
    Telemetry.mark_sql_snapshot_error(:econnrefused, 2)
    assert Telemetry.mark_sql_snapshot_success(5) == :recovered
  end

  test "consecutive successes return :ok" do
    assert Telemetry.mark_sql_snapshot_success(5) == :ok
    assert Telemetry.mark_sql_snapshot_success(7) == :ok
  end

  test "success after success returns :ok even if a stale error reason is in ets" do
    # Simulate a fresh start where status has never been :error.
    Telemetry.mark_sql_snapshot_success(3)
    assert Telemetry.mark_sql_snapshot_success(4) == :ok
  end
end
