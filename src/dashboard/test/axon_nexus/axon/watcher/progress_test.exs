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

  test "oversized_for_current_budget is reported as oversized in workspace status" do
    with_sql_gateway_rows([["indexed", 2], ["oversized_for_current_budget", 3], ["pending", 1]], fn ->
      status = Progress.get_status("progress-test")

      assert status["oversized"] == 3
      assert status["total"] == 6
      assert status["completed"] == 5
    end)
  end

  test "oversized_for_current_budget is reported in project aggregates" do
    with_sql_gateway_rows(
      [
        ["alpha", "indexed", 2],
        ["alpha", "oversized_for_current_budget", 3],
        ["beta", "pending", 1]
      ],
      fn ->
        projects = Progress.list_projects("progress-test")
        alpha = Enum.find(projects, &(&1.project_code == "alpha"))

        assert alpha.oversized == 3
        assert alpha.completed == 5
        assert alpha.total == 5
      end
    )
  end

  test "workspace status exposes graph_ready and vector_ready counters" do
    with_sql_gateway_responses(
      [
        {"COALESCE(status", [["indexed", 2], ["pending", 1]]},
        {"COALESCE(file_stage", [["graph_indexed", 2], ["promoted", 1]]},
        {"SUM(CASE WHEN f.graph_ready", [[2, 1]]},
        {"SELECT COUNT(*) FROM File WHERE vector_ready = TRUE", [[5]]},
        {"SELECT COUNT(*) FROM ChunkEmbedding", [[8]]},
        {"COUNT(DISTINCT anchor_type || ':' || anchor_id) FROM GraphEmbedding", [[3]]},
        {"SELECT COUNT(*) FROM Symbol", [[10]]},
        {"links_count", [[12]]}
      ],
      fn ->
        status = Progress.get_status("progress-test")

        assert status["graph_ready"] == 2
        assert status["vector_ready"] == 1
        assert status["vector_ready_file_raw"] == 5
        assert status["chunk_embeddings_count"] == 8
        assert status["graph_embeddings_count"] == 3
        assert status["stage_graph_indexed"] == 2
        assert status["stage_promoted"] == 1
      end
    )
  end

  test "workspace status derives vector readiness against the configured active chunk model" do
    previous = System.get_env("AXON_CHUNK_MODEL_ID")
    System.put_env("AXON_CHUNK_MODEL_ID", "chunk-bge-large-en-v1.5-1024")

    on_exit(fn ->
      if previous do
        System.put_env("AXON_CHUNK_MODEL_ID", previous)
      else
        System.delete_env("AXON_CHUNK_MODEL_ID")
      end
    end)

    with_sql_gateway_responses(
      [
        {"COALESCE(status", [["indexed", 2], ["pending", 1]]},
        {"COALESCE(file_stage", [["graph_indexed", 2], ["promoted", 1]]},
        {"chunk-bge-large-en-v1.5-1024", [[2, 1]]},
        {"SELECT COUNT(*) FROM File WHERE vector_ready = TRUE", [[5]]},
        {"SELECT COUNT(*) FROM ChunkEmbedding", [[8]]},
        {"COUNT(DISTINCT anchor_type || ':' || anchor_id) FROM GraphEmbedding", [[3]]},
        {"SELECT COUNT(*) FROM Symbol", [[10]]},
        {"links_count", [[12]]}
      ],
      fn ->
        status = Progress.get_status("progress-test")

        assert status["graph_ready"] == 2
        assert status["vector_ready"] == 1
      end
    )
  end

  test "snapshot derives coherent workspace projects and reasons from one SQL payload" do
    with_sql_gateway_responses(
      [
        {:default,
         [
           ["workspace_status", nil, "indexed", 2, nil],
           ["workspace_status", nil, "indexed_degraded", 1, nil],
           ["workspace_status", nil, "pending", 1, nil],
           ["workspace_status", nil, "oversized_for_current_budget", 3, nil],
           ["workspace_stage", nil, "graph_indexed", 3, nil],
           ["workspace_stage", nil, "promoted", 1, nil],
           ["workspace_stage", nil, "claimed", 1, nil],
           ["workspace_ready", nil, "ready", 3, 2],
           ["project_status", "alpha", "indexed", 2, nil],
           ["project_status", "alpha", "indexed_degraded", 1, nil],
           ["project_status", "beta", "pending", 1, nil],
           ["project_status", "beta", "oversized_for_current_budget", 3, nil],
           ["project_ready", "alpha", "ready", 3, 2],
           ["project_ready", "beta", "ready", 0, 0],
           ["backlog_reason", nil, "claimed_for_indexing", 1, nil]
         ]},
        {"FROM soll.ProjectCodeRegistry", [["alpha", "Alpha Project"], ["beta", "Beta Project"]]},
        {"AS indexed_graph_ready", [[2, 0, 1, 0, 2, 0, 0, 1]]},
        {"SELECT COUNT(*) FROM File WHERE vector_ready = TRUE", [[4]]},
        {"SELECT COUNT(*) FROM ChunkEmbedding", [[6]]},
        {"COUNT(DISTINCT anchor_type || ':' || anchor_id) FROM GraphEmbedding", [[1]]},
        {"SELECT COUNT(*) FROM Symbol", [[7]]},
        {"links_count", [[9]]}
      ],
      fn ->
        snapshot = Progress.get_snapshot("progress-test")
        alpha = Enum.find(snapshot.projects, &(&1.project_code == "alpha"))
        beta = Enum.find(snapshot.projects, &(&1.project_code == "beta"))

        assert snapshot.workspace["known"] == 7
        assert snapshot.workspace["completed"] == 6
        assert snapshot.workspace["oversized"] == 3
        assert snapshot.workspace["completed_oversized"] == 3
        assert snapshot.workspace["graph_ready"] == 3
        assert snapshot.workspace["vector_ready"] == 2
        assert snapshot.workspace["semantic_coverage"] == 2
        assert snapshot.workspace["vector_ready_file_raw"] == 4
        assert snapshot.workspace["chunk_embeddings_count"] == 6
        assert snapshot.workspace["graph_embeddings_count"] == 1
        assert snapshot.workspace["stage_graph_indexed"] == 3

        assert alpha.known == 3
        assert alpha.completed == 3
        assert alpha.project_name == "Alpha Project"
        assert alpha.display_name == "Alpha Project (alpha)"
        assert alpha.graph_ready == 3
        assert alpha.vector_ready == 2

        assert beta.known == 4
        assert beta.project_name == "Beta Project"
        assert beta.display_name == "Beta Project (beta)"
        assert beta.pending == 1
        assert beta.oversized == 3
        assert beta.completed == 3

        assert Enum.at(snapshot.reasons, 0).reason == "claimed_for_indexing"
        assert Enum.at(snapshot.reasons, 0).count == 1
      end
    )
  end

  defp with_sql_gateway_rows(rows, fun) do
    with_sql_gateway_responses([{:default, rows}], fun)
  end

  defp with_sql_gateway_responses(routes, fun) do
    :inets.start()
    :ssl.start()
    port = random_port()

    {:ok, listener} =
      :gen_tcp.listen(port, [:binary, packet: :raw, active: false, reuseaddr: true])

    previous = Application.get_env(:axon_dashboard, Axon.Watcher.SqlGateway, [])

    Application.put_env(
      :axon_dashboard,
      Axon.Watcher.SqlGateway,
      Keyword.put(previous, :url, "http://127.0.0.1:#{port}/sql")
    )

    parent = self()

    task =
      Task.async(fn ->
        accept_loop(listener, routes, parent)
      end)

    try do
      fun.()
    after
      Application.put_env(:axon_dashboard, Axon.Watcher.SqlGateway, previous)
      send(task.pid, :stop)
      Task.await(task, 5_000)
    end
  end

  defp accept_loop(listener, routes, parent) do
    receive do
      :stop ->
        :gen_tcp.close(listener)
        :ok
    after
      50 ->
        case :gen_tcp.accept(listener, 200) do
          {:ok, socket} ->
            {:ok, request} = :gen_tcp.recv(socket, 0, 5_000)
            body = response_body_for_request(request, routes)

            response = [
              "HTTP/1.1 200 OK\r\n",
              "content-type: application/json\r\n",
              "content-length: #{byte_size(body)}\r\n",
              "connection: close\r\n\r\n",
              body
            ]

            :ok = :gen_tcp.send(socket, response)
            :gen_tcp.close(socket)
            accept_loop(listener, routes, parent)

          {:error, :timeout} ->
            if Process.alive?(parent) do
              accept_loop(listener, routes, parent)
            else
              :gen_tcp.close(listener)
              :ok
            end
        end
    end
  end

  defp response_body_for_request(request, routes) do
    request = IO.iodata_to_binary(request)

    query =
      case Regex.run(~r/\r\n\r\n(?<body>\{.*\})/s, request, capture: :all_names) do
        [body] ->
          case Jason.decode(body) do
            {:ok, %{"query" => query}} -> query
            _ -> ""
          end

        _ ->
          ""
      end

    normalized_query = normalize_sql(query)

    if String.contains?(normalized_query, "indexed_graph_ready") and
         String.contains?(normalized_query, "indexed_degraded_vector_missing") do
      Jason.encode!([[2, 0, 1, 0, 2, 0, 0, 1]])
    else

      routes
      |> Enum.reject(fn {needle, _rows} -> needle == :default end)
      |> Enum.find_value(fn
        {needle, rows} when is_binary(needle) ->
          if String.contains?(normalized_query, normalize_sql(needle)), do: Jason.encode!(rows), else: nil
      end)
      |> Kernel.||(default_route_body(routes))
    end
  end

  defp default_route_body(routes) do
    Enum.find_value(routes, "[]", fn
      {:default, rows} -> Jason.encode!(rows)
      _route -> nil
    end)
    |> Kernel.||("[]")
  end

  defp random_port do
    45_000 + rem(:erlang.unique_integer([:positive]), 10_000)
  end

  defp normalize_sql(value) do
    value
    |> to_string()
    |> String.downcase()
    |> String.replace(~r/\s+/u, " ")
    |> String.trim()
  end
end
