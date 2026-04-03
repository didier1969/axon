# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress using DuckDB as the sole source of truth.
  """

  alias Axon.Watcher.SqlGateway
  alias Axon.Watcher.Telemetry

  require Logger

  @terminal_statuses ["indexed", "indexed_degraded", "skipped", "deleted"]
  @oversized_status "oversized_for_current_budget"

  def get_snapshot(_repo_slug) do
    rows = query_rows(snapshot_query())

    workspace_counts = rows |> section_rows("workspace_status") |> normalize_counts()
    stage_counts = rows |> section_rows("workspace_stage") |> normalize_counts()
    {graph_ready, vector_ready} = rows |> section_rows("workspace_ready") |> decode_snapshot_ready_pair()

    readiness_by_project =
      rows
      |> section_rows("project_ready")
      |> Enum.reduce(%{}, fn
        [_section, slug, _key, graph_ready, vector_ready], acc ->
          Map.put(acc, slug, %{graph_ready: decode_integer(graph_ready), vector_ready: decode_integer(vector_ready)})

        _row, acc ->
          acc
      end)

    projects =
      rows
      |> section_rows("project_status")
      |> Enum.group_by(&Enum.at(&1, 1))
      |> Enum.map(fn {slug, project_rows} ->
        counts =
          Enum.into(project_rows, %{}, fn [_section, _slug, status, count, _secondary_count] ->
            {status, decode_integer(count)}
          end)

        total = Enum.sum(Map.values(counts))
        completed = completed_total(counts)
        readiness = Map.get(readiness_by_project, slug, %{graph_ready: 0, vector_ready: 0})

        %{
          slug: slug,
          known: total,
          total: total,
          completed: completed,
          pending: Map.get(counts, "pending", 0),
          indexing: Map.get(counts, "indexing", 0),
          degraded: Map.get(counts, "indexed_degraded", 0),
          oversized: oversized_total(counts),
          skipped: Map.get(counts, "skipped", 0),
          graph_ready: readiness.graph_ready,
          vector_ready: readiness.vector_ready,
          progress: percentage(completed, total),
          readiness: readiness_label(counts, total)
        }
      end)
      |> Enum.sort_by(fn project -> {-project.known, project.slug} end, :asc)

    reasons =
      rows
      |> section_rows("backlog_reason")
      |> Enum.map(fn [_section, _scope, reason, count, _secondary_count] ->
        %{reason: reason, count: decode_integer(count), label: humanize_reason(reason)}
      end)
      |> Enum.sort_by(&{-&1.count, &1.reason}, :asc)
      |> Enum.take(8)

    total = Enum.sum(Map.values(workspace_counts))
    indexed = Map.get(workspace_counts, "indexed", 0)
    degraded = Map.get(workspace_counts, "indexed_degraded", 0)
    skipped = Map.get(workspace_counts, "skipped", 0)
    terminal = indexed + degraded + skipped + Map.get(workspace_counts, "deleted", 0)
    progress = percentage(terminal, total)

    workspace = %{
      "status" => workspace_state(workspace_counts, total),
      "progress" => progress,
      "synced" => indexed + degraded,
      "total" => total,
      "indexed" => indexed,
      "indexed_degraded" => degraded,
      "pending" => Map.get(workspace_counts, "pending", 0),
      "indexing" => Map.get(workspace_counts, "indexing", 0),
      "oversized" => oversized_total(workspace_counts),
      "skipped" => skipped,
      "deleted" => Map.get(workspace_counts, "deleted", 0),
      "graph_ready" => graph_ready,
      "vector_ready" => vector_ready,
      "stage_promoted" => Map.get(stage_counts, "promoted", 0),
      "stage_claimed" => Map.get(stage_counts, "claimed", 0),
      "stage_writer_pending_commit" => Map.get(stage_counts, "writer_pending_commit", 0),
      "stage_graph_indexed" => Map.get(stage_counts, "graph_indexed", 0),
      "known" => total,
      "completed" => terminal,
      "last_update" => DateTime.utc_now() |> DateTime.to_iso8601()
    }

    %{workspace: workspace, projects: projects, reasons: reasons}
  end

  def get_status(_repo_slug) do
    counts =
      "SELECT COALESCE(status, 'unknown'), count(*) FROM File GROUP BY 1;"
      |> query_rows()
      |> normalize_counts()

    stage_counts =
      "SELECT COALESCE(file_stage, 'unknown'), count(*) FROM File GROUP BY 1;"
      |> query_rows()
      |> normalize_counts()

    {graph_ready, vector_ready} =
      "SELECT SUM(CASE WHEN graph_ready THEN 1 ELSE 0 END), SUM(CASE WHEN vector_ready THEN 1 ELSE 0 END) FROM File;"
      |> query_rows()
      |> decode_ready_pair()

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
      "oversized" => oversized_total(counts),
      "skipped" => skipped,
      "deleted" => Map.get(counts, "deleted", 0),
      "graph_ready" => graph_ready,
      "vector_ready" => vector_ready,
      "stage_promoted" => Map.get(stage_counts, "promoted", 0),
      "stage_claimed" => Map.get(stage_counts, "claimed", 0),
      "stage_writer_pending_commit" => Map.get(stage_counts, "writer_pending_commit", 0),
      "stage_graph_indexed" => Map.get(stage_counts, "graph_indexed", 0),
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
    readiness_by_project =
      "SELECT COALESCE(project_slug, '(unscoped)'), SUM(CASE WHEN graph_ready THEN 1 ELSE 0 END), SUM(CASE WHEN vector_ready THEN 1 ELSE 0 END) FROM File GROUP BY 1;"
      |> query_rows()
      |> Enum.reduce(%{}, fn
        [slug, graph_ready, vector_ready], acc ->
          Map.put(acc, slug, %{graph_ready: decode_integer(graph_ready), vector_ready: decode_integer(vector_ready)})

        _row, acc ->
          acc
      end)

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
      readiness = Map.get(readiness_by_project, slug, %{graph_ready: 0, vector_ready: 0})

      %{
        slug: slug,
        known: total,
        total: total,
        completed: completed,
        pending: Map.get(counts, "pending", 0),
        indexing: Map.get(counts, "indexing", 0),
        degraded: Map.get(counts, "indexed_degraded", 0),
        oversized: oversized_total(counts),
        skipped: Map.get(counts, "skipped", 0),
        graph_ready: readiness.graph_ready,
        vector_ready: readiness.vector_ready,
        progress: percentage(completed, total),
        readiness: readiness_label(counts, total)
      }
    end)
    |> Enum.sort_by(
      fn project ->
        {-project.known, project.slug}
      end,
      :asc
    )
  end

  def list_backlog_reasons(_repo_slug) do
    "SELECT COALESCE(status_reason, 'unknown'), count(*) FROM File WHERE status IN ('pending', 'indexing', 'indexed_degraded', '#{@oversized_status}') GROUP BY 1 ORDER BY 2 DESC LIMIT 8;"
    |> query_rows()
    |> Enum.map(fn [reason, count] ->
      %{reason: reason, count: decode_integer(count), label: humanize_reason(reason)}
    end)
  end

  def get_file_mtime(_, _), do: 0
  def save_file_mtime(_, _, _), do: :ok

  defp query_rows(query) do
    started_at = System.monotonic_time(:millisecond)

    case SqlGateway.query_json(query) do
      {:ok, json} ->
        Telemetry.mark_sql_snapshot_success(System.monotonic_time(:millisecond) - started_at)

        case Jason.decode(json) do
          {:ok, rows} when is_list(rows) -> rows
          _ -> []
        end

      {:error, reason} ->
        duration_ms = System.monotonic_time(:millisecond) - started_at
        Telemetry.mark_sql_snapshot_error(reason, duration_ms)
        Logger.warning("[cockpit] SQL snapshot query failed after #{duration_ms}ms: #{inspect(reason)}")
        []
    end
  end

  defp snapshot_query do
    """
    WITH normalized_file AS (
      SELECT
        COALESCE(project_slug, '(unscoped)') AS project_slug,
        COALESCE(status, 'unknown') AS status,
        COALESCE(file_stage, 'unknown') AS file_stage,
        COALESCE(status_reason, 'unknown') AS status_reason,
        graph_ready,
        vector_ready
      FROM File
    )
    SELECT
      'workspace_status' AS section,
      NULL AS scope,
      status AS key,
      count(*) AS primary_count,
      NULL AS secondary_count
    FROM normalized_file
    GROUP BY 1, 2, 3

    UNION ALL

    SELECT
      'workspace_stage' AS section,
      NULL AS scope,
      file_stage AS key,
      count(*) AS primary_count,
      NULL AS secondary_count
    FROM normalized_file
    GROUP BY 1, 2, 3

    UNION ALL

    SELECT
      'workspace_ready' AS section,
      NULL AS scope,
      'ready' AS key,
      SUM(CASE WHEN graph_ready THEN 1 ELSE 0 END) AS primary_count,
      SUM(CASE WHEN vector_ready THEN 1 ELSE 0 END) AS secondary_count
    FROM normalized_file

    UNION ALL

    SELECT
      'project_status' AS section,
      project_slug AS scope,
      status AS key,
      count(*) AS primary_count,
      NULL AS secondary_count
    FROM normalized_file
    GROUP BY 1, 2, 3

    UNION ALL

    SELECT
      'project_ready' AS section,
      project_slug AS scope,
      'ready' AS key,
      SUM(CASE WHEN graph_ready THEN 1 ELSE 0 END) AS primary_count,
      SUM(CASE WHEN vector_ready THEN 1 ELSE 0 END) AS secondary_count
    FROM normalized_file
    GROUP BY 1, 2, 3

    UNION ALL

    SELECT
      'backlog_reason' AS section,
      NULL AS scope,
      status_reason AS key,
      count(*) AS primary_count,
      NULL AS secondary_count
    FROM normalized_file
    WHERE status IN ('pending', 'indexing', 'indexed_degraded', '#{@oversized_status}')
    GROUP BY 1, 2, 3
    """
  end

  defp section_rows(rows, section) do
    Enum.filter(rows, fn
      [^section | _rest] -> true
      _row -> false
    end)
  end

  defp normalize_counts(rows) do
    Enum.into(rows, %{}, fn
      [status, count] ->
        {status, decode_integer(count)}

      [_section, _scope, status, count, _secondary_count] ->
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

  defp decode_ready_pair([[graph_ready, vector_ready] | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_ready_pair(_rows), do: {0, 0}

  defp decode_snapshot_ready_pair([[_section, _scope, _key, graph_ready, vector_ready] | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_snapshot_ready_pair(_rows), do: {0, 0}

  defp percentage(_numerator, 0), do: 0
  defp percentage(numerator, denominator), do: round(numerator / denominator * 100)

  defp completed_total(counts) do
    Enum.reduce(@terminal_statuses, 0, fn status, acc -> acc + Map.get(counts, status, 0) end)
  end

  defp oversized_total(counts), do: Map.get(counts, @oversized_status, 0)

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
