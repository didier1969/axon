# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.ProgressTest do
  use ExUnit.Case, async: false

  alias Axon.Watcher.Progress

  test "indexed_degraded counts as synced progress" do
    with_sql_gateway_rows([["indexed", 2], ["indexed_degraded", 1], ["pending", 1]], fn ->
      status = Progress.get_status("progress-test")

      assert status["status"] == "queued"
      assert status["synced"] == 3
      assert status["total"] == 4
      assert status["progress"] == 75
    end)
  end

  test "directory stats count indexed_degraded as completed" do
    with_sql_gateway_rows(
      [
        ["alpha", "indexed", 2],
        ["alpha", "indexed_degraded", 1],
        ["alpha", "pending", 1],
        ["beta", "indexed_degraded", 2]
      ],
      fn ->
        stats = Progress.get_directory_stats("progress-test")

        assert stats["alpha"].completed == 3
        assert stats["alpha"].total == 4
        assert stats["beta"].completed == 2
        assert stats["beta"].total == 2
      end
    )
  end

  defp with_sql_gateway_rows(rows, fun) do
    :inets.start()
    :ssl.start()
    body = Jason.encode!(rows)
    port = random_port()

    {:ok, listener} =
      :gen_tcp.listen(port, [:binary, packet: :raw, active: false, reuseaddr: true])

    previous = Application.get_env(:axon_dashboard, Axon.Watcher.SqlGateway, [])

    Application.put_env(
      :axon_dashboard,
      Axon.Watcher.SqlGateway,
      Keyword.put(previous, :url, "http://127.0.0.1:#{port}/sql")
    )

    task =
      Task.async(fn ->
        {:ok, socket} = :gen_tcp.accept(listener)
        {:ok, _request} = :gen_tcp.recv(socket, 0, 5_000)

        response = [
          "HTTP/1.1 200 OK\r\n",
          "content-type: application/json\r\n",
          "content-length: #{byte_size(body)}\r\n",
          "connection: close\r\n\r\n",
          body
        ]

        :ok = :gen_tcp.send(socket, response)
        :gen_tcp.close(socket)
        :gen_tcp.close(listener)
      end)

    try do
      fun.()
    after
      Application.put_env(:axon_dashboard, Axon.Watcher.SqlGateway, previous)
      Task.await(task, 5_000)
    end
  end

  defp random_port do
    45_000 + rem(:erlang.unique_integer([:positive]), 10_000)
  end
end
