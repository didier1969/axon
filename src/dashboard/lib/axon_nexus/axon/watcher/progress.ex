# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress using DuckDB as the sole source of truth.
  """

  alias Axon.Watcher.SqlGateway

  @terminal_statuses ["indexed", "indexed_degraded", "skipped", "deleted"]
  def get_status(_repo_slug) do
    counts =
      "SELECT COALESCE(status, 'unknown'), count(*) FROM File GROUP BY 1;"
      |> query_rows()
      |> normalize_counts()

    total = Enum.sum(Map.values(counts))
    indexed = Map.get(counts, "indexed", 0)
    degraded = Map.get(counts, "indexed_degraded", 0)
    skipped = Map.get(counts, "skipped", 0)
    terminal = indexed + degraded + skipped + Map.get(counts, "deleted", 0)
    progress = percentage(terminal, total)

    %{
      "status" => workspace_state(counts, total),
      "progress" => progress,
      "synced" => indexed + degraded,
      "total" => total,
      "indexed" => indexed,
      "indexed_degraded" => degraded,
      "pending" => Map.get(counts, "pending", 0),
      "indexing" => Map.get(counts, "indexing", 0),
      "oversized" => Map.get(counts, "oversized", 0),
      "skipped" => skipped,
      "deleted" => Map.get(counts, "deleted", 0),
      "known" => total,
      "completed" => terminal,
      "last_update" => DateTime.utc_now() |> DateTime.to_iso8601()
    }
  end

  def get_directory_stats(repo_slug) do
    repo_slug
    |> list_projects()
    |> Enum.into(%{}, fn project ->
      {project.slug,
       %{
         total: project.total,
         completed: project.completed,
         failed: project.degraded + project.oversized,
         last_update: DateTime.utc_now()
       }}
    end)
  end

  def list_projects(_repo_slug) do
    "SELECT COALESCE(project_slug, '(unscoped)'), COALESCE(status, 'unknown'), count(*) FROM File GROUP BY 1, 2;"
    |> query_rows()
    |> Enum.group_by(&Enum.at(&1, 0))
    |> Enum.map(fn {slug, rows} ->
      counts =
        Enum.into(rows, %{}, fn [_slug, status, count] ->
          {status, decode_integer(count)}
        end)

      total = Enum.sum(Map.values(counts))
      completed = completed_total(counts)

      %{
        slug: slug,
        total: total,
        completed: completed,
        pending: Map.get(counts, "pending", 0),
        indexing: Map.get(counts, "indexing", 0),
        degraded: Map.get(counts, "indexed_degraded", 0),
        oversized: Map.get(counts, "oversized", 0),
        skipped: Map.get(counts, "skipped", 0),
        progress: percentage(completed, total),
        readiness: readiness_label(counts, total)
      }
    end)
    |> Enum.sort_by(
      fn project ->
        {project.indexing + project.pending, project.total, project.slug}
      end,
      :desc
    )
  end

  def list_backlog_reasons(_repo_slug) do
    "SELECT COALESCE(status_reason, 'unknown'), count(*) FROM File WHERE status IN ('pending', 'indexing', 'indexed_degraded', 'oversized') GROUP BY 1 ORDER BY 2 DESC LIMIT 8;"
    |> query_rows()
    |> Enum.map(fn [reason, count] ->
      %{reason: reason, count: decode_integer(count), label: humanize_reason(reason)}
    end)
  end

  def get_file_mtime(_, _), do: 0
  def save_file_mtime(_, _, _), do: :ok

  defp query_rows(query) do
    case SqlGateway.query_json(query) do
      {:ok, json} ->
        case Jason.decode(json) do
          {:ok, rows} when is_list(rows) -> rows
          _ -> []
        end

      _ ->
        []
    end
  end

  defp normalize_counts(rows) do
    Enum.into(rows, %{}, fn
      [status, count] ->
        {status, decode_integer(count)}

      row when is_list(row) and length(row) >= 2 ->
        {Enum.at(row, 0), decode_integer(Enum.at(row, 1))}
    end)
  end

  defp decode_integer(value) when is_integer(value), do: value
  defp decode_integer(value) when is_float(value), do: round(value)

  defp decode_integer(value) when is_binary(value) do
    case Integer.parse(value) do
      {parsed, _} -> parsed
      :error -> 0
    end
  end

  defp decode_integer(_value), do: 0

  defp percentage(_numerator, 0), do: 0
  defp percentage(numerator, denominator), do: round(numerator / denominator * 100)

  defp completed_total(counts) do
    Enum.reduce(@terminal_statuses, 0, fn status, acc -> acc + Map.get(counts, status, 0) end)
  end

  defp workspace_state(_counts, 0), do: "connecting"

  defp workspace_state(counts, total) do
    cond do
      Map.get(counts, "indexing", 0) > 0 -> "indexing"
      Map.get(counts, "pending", 0) > 0 -> "queued"
      completed_total(counts) >= total -> "ready"
      true -> "live"
    end
  end

  defp readiness_label(_counts, 0), do: "empty"

  defp readiness_label(counts, total) do
    completed = completed_total(counts)
    degraded = Map.get(counts, "indexed_degraded", 0)
    pending = Map.get(counts, "pending", 0) + Map.get(counts, "indexing", 0)

    cond do
      completed == total and degraded == 0 -> "ready"
      completed > 0 and pending == 0 -> "partial"
      completed > 0 -> "warming"
      true -> "queued"
    end
  end

  defp humanize_reason(reason) do
    reason
    |> to_string()
    |> String.replace("_", " ")
    |> String.split()
    |> Enum.map_join(" ", &String.capitalize/1)
  end
end
