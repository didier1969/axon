defmodule LiveView.Witness.OracleTest do
  use ExUnit.Case, async: true
  use Plug.Test

  alias LiveView.Witness.Oracle

  @opts Oracle.init([])

  test "returns received JSON for /liveview_witness/diagnose" do
    conn =
      conn(:post, "/liveview_witness/diagnose", ~s({"error": "500", "at": "index"}))
      |> put_req_header("content-type", "application/json")
      |> Oracle.call(@opts)

    assert conn.state == :sent
    assert conn.status == 200
    assert conn.resp_body == ~s({"status":"received"})
    assert conn.halted
  end

  test "ignores other paths" do
    conn =
      conn(:get, "/other")
      |> Oracle.call(@opts)

    refute conn.halted
    assert conn.status == nil
  end
end
