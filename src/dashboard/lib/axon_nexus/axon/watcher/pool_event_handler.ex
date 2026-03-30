defmodule Axon.Watcher.PoolEventHandler do
  @moduledoc false

  def process_pending(batch_files) do
    files_to_index =
      Enum.map(batch_files, fn f ->
        %{"path" => f["path"], "trace_id" => f["trace_id"], "priority" => f["priority"] || 100}
      end)

    %{"batch" => files_to_index}
    |> Axon.Watcher.IndexingWorker.new()
    |> Oban.insert()

    :ok
  end

  def process_indexed(payload) do
    path = payload["path"]
    final_status = if payload["status"] == "ok", do: "indexed", else: payload["status"]
    project_id = extract_project(path)

    if final_status == "indexed" do
      Axon.Watcher.StatsCache.increment_file_stats(project_id, %{
        completed: 1,
        symbols: payload["symbol_count"] || 0,
        relations: payload["relation_count"] || 0
      })
    end

    if payload["t0"] > 0 do
      Axon.Watcher.Tracer.record_trace(
        payload["trace_id"] || "none",
        path,
        payload["t0"],
        payload["t1"] || 0,
        payload["t2"] || 0,
        payload["t3"] || 0,
        payload["t4"] || 0
      )
    end

    :ok
  end

  defp extract_project(path) do
    case String.split(path, "/projects/") do
      [_, tail] -> String.split(tail, "/") |> List.first()
      _ -> "global"
    end
  end
end
