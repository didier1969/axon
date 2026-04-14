# Copyright (c) Didier Stadelmann. All rights reserved.

defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress using DuckDB as the sole source of truth.
  """

  alias Axon.Watcher.SqlGateway
  alias Axon.Watcher.Telemetry

  require Logger

  @terminal_statuses ["indexed", "indexed_degraded", "skipped", "deleted", "oversized_for_current_budget"]
  @oversized_status "oversized_for_current_budget"
  @default_chunk_model_id "chunk-bge-large-en-v1.5-1024"

  def get_snapshot(_repo_code) do
    rows = query_rows(snapshot_query())
    soll_coverage = query_rows(soll_coverage_query())
    soll_revision = query_rows(soll_revision_query())
    flow_breakdown = query_rows(workspace_pipeline_breakdown_query()) |> decode_flow_breakdown()
    global_graph_vectors = query_rows(global_graph_vector_query())
    global_chunk_embeddings = query_rows(global_chunk_embedding_query())
    global_file_vector_flags = query_rows(global_file_vector_flag_query())
    global_nodes = query_rows(global_nodes_query())
    global_links = query_rows(global_links_query())
    project_nodes = query_rows(project_nodes_query()) |> decode_scope_counts()
    project_links = query_rows(project_links_query()) |> decode_scope_counts()
    project_graph_vectors = query_rows(project_graph_vector_query()) |> decode_scope_counts()
    project_names = query_rows(project_names_query()) |> decode_scope_names()

    workspace_counts = rows |> section_rows("workspace_status") |> normalize_counts()
    stage_counts = rows |> section_rows("workspace_stage") |> normalize_counts()
    {graph_ready, vector_ready} = rows |> section_rows("workspace_ready") |> decode_snapshot_ready_pair()

    readiness_by_project =
      rows
      |> section_rows("project_ready")
      |> Enum.reduce(%{}, fn
        [_section, project_code, _key, graph_ready, vector_ready], acc ->
          Map.put(acc, project_code, %{graph_ready: decode_integer(graph_ready), vector_ready: decode_integer(vector_ready)})

        _row, acc ->
          acc
      end)

    projects =
      rows
      |> section_rows("project_status")
      |> Enum.group_by(&Enum.at(&1, 1))
      |> Enum.map(fn {project_code, project_rows} ->
        counts =
          Enum.into(project_rows, %{}, fn [_section, _project_code, status, count, _secondary_count] ->
            {status, decode_integer(count)}
          end)

        total = Enum.sum(Map.values(counts))
        completed = completed_total(counts)
        readiness = Map.get(readiness_by_project, project_code, %{graph_ready: 0, vector_ready: 0})
        project_nodes_count = Map.get(project_nodes, project_code, 0)
        project_graph_vectors_count = Map.get(project_graph_vectors, project_code, 0)

        %{
          project_code: project_code,
          project_name: Map.get(project_names, project_code, project_code),
          display_name: display_project_name(Map.get(project_names, project_code, project_code), project_code),
          known: total,
          total: total,
          completed: completed,
          pending: Map.get(counts, "pending", 0),
          indexing: Map.get(counts, "indexing", 0),
          degraded: Map.get(counts, "indexed_degraded", 0),
          oversized: oversized_total(counts),
          skipped: Map.get(counts, "skipped", 0),
          graph_ready: readiness.graph_ready,
          graph_ready_pct: percentage(readiness.graph_ready, total),
          vector_ready: readiness.vector_ready,
          vector_ready_file: readiness.vector_ready,
          vector_ready_file_pct: percentage(readiness.vector_ready, total),
          vector_ready_graph: project_graph_vectors_count,
          vector_ready_graph_pct: percentage(project_graph_vectors_count, project_nodes_count),
          nodes_count: project_nodes_count,
          links_count: Map.get(project_links, project_code, 0),
          progress: percentage(completed, total),
          readiness: readiness_label(counts, total)
        }
      end)
      |> Enum.sort_by(fn project -> {-project.known, project.project_code} end, :asc)

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
    oversized = oversized_total(workspace_counts)
    terminal = completed_total(workspace_counts)
    progress = percentage(terminal, total)

    workspace = %{
      "status" => workspace_state(workspace_counts, total),
      "progress" => progress,
      "global_indexation_pct" => progress,
      "synced" => indexed + degraded,
      "total" => total,
      "indexed" => indexed,
      "indexed_degraded" => degraded,
      "pending" => Map.get(workspace_counts, "pending", 0),
      "indexing" => Map.get(workspace_counts, "indexing", 0),
      "oversized" => oversized,
      "skipped" => skipped,
      "deleted" => Map.get(workspace_counts, "deleted", 0),
      "completed_indexed" => indexed,
      "completed_indexed_degraded" => degraded,
      "completed_skipped" => skipped,
      "completed_deleted" => Map.get(workspace_counts, "deleted", 0),
      "completed_oversized" => oversized,
      "graph_ready" => graph_ready,
      "graph_ready_pct" => percentage(graph_ready, total),
      "vector_ready" => vector_ready,
      "vector_ready_file" => vector_ready,
      "semantic_coverage" => vector_ready,
      "semantic_coverage_pct" => percentage(vector_ready, total),
      "vector_ready_file_pct" => percentage(vector_ready, total),
      "vector_ready_file_raw" => decode_single_count(global_file_vector_flags),
      "vector_ready_graph" => decode_single_count(global_graph_vectors),
      "graph_embeddings_count" => decode_single_count(global_graph_vectors),
      "chunk_embeddings_count" => decode_single_count(global_chunk_embeddings),
      "nodes_count" => decode_single_count(global_nodes),
      "vector_ready_graph_pct" =>
        percentage(decode_single_count(global_graph_vectors), decode_single_count(global_nodes)),
      "links_count" => decode_single_count(global_links),
      "stage_promoted" => Map.get(stage_counts, "promoted", 0),
      "stage_claimed" => Map.get(stage_counts, "claimed", 0),
      "stage_writer_pending_commit" => Map.get(stage_counts, "writer_pending_commit", 0),
      "stage_graph_indexed" => Map.get(stage_counts, "graph_indexed", 0),
      "known" => total,
      "completed" => terminal,
      "indexed_graph_ready" => Map.get(flow_breakdown, "indexed_graph_ready", 0),
      "indexed_graph_missing" => Map.get(flow_breakdown, "indexed_graph_missing", 0),
      "indexed_degraded_graph_ready" => Map.get(flow_breakdown, "indexed_degraded_graph_ready", 0),
      "indexed_degraded_graph_missing" => Map.get(flow_breakdown, "indexed_degraded_graph_missing", 0),
      "indexed_vector_ready" => Map.get(flow_breakdown, "indexed_vector_ready", 0),
      "indexed_vector_missing" => Map.get(flow_breakdown, "indexed_vector_missing", 0),
      "indexed_degraded_vector_ready" => Map.get(flow_breakdown, "indexed_degraded_vector_ready", 0),
      "indexed_degraded_vector_missing" => Map.get(flow_breakdown, "indexed_degraded_vector_missing", 0),
      "soll_done" => decode_soll_metric(soll_coverage, 1),
      "soll_partial" => decode_soll_metric(soll_coverage, 2),
      "soll_missing" => decode_soll_metric(soll_coverage, 3),
      "soll_last_revision" => decode_soll_revision(soll_revision),
      "last_update" => DateTime.utc_now() |> DateTime.to_iso8601()
    }

    %{workspace: workspace, projects: projects, reasons: reasons}
  end

  def get_status(_repo_code) do
    flow_breakdown = query_rows(workspace_pipeline_breakdown_query()) |> decode_flow_breakdown()

    counts =
      "SELECT COALESCE(status, 'unknown'), count(*) FROM File GROUP BY 1;"
      |> query_rows()
      |> normalize_counts()

    stage_counts =
      "SELECT COALESCE(file_stage, 'unknown'), count(*) FROM File GROUP BY 1;"
      |> query_rows()
      |> normalize_counts()

    {graph_ready, vector_ready} =
      workspace_ready_query()
      |> query_rows()
      |> decode_ready_pair()

    total = Enum.sum(Map.values(counts))
    indexed = Map.get(counts, "indexed", 0)
    degraded = Map.get(counts, "indexed_degraded", 0)
    skipped = Map.get(counts, "skipped", 0)
    oversized = oversized_total(counts)
    terminal = completed_total(counts)
    progress = percentage(terminal, total)

    %{
      "status" => workspace_state(counts, total),
      "progress" => progress,
      "global_indexation_pct" => progress,
      "synced" => indexed + degraded,
      "total" => total,
      "indexed" => indexed,
      "indexed_degraded" => degraded,
      "pending" => Map.get(counts, "pending", 0),
      "indexing" => Map.get(counts, "indexing", 0),
      "oversized" => oversized,
      "skipped" => skipped,
      "deleted" => Map.get(counts, "deleted", 0),
      "completed_indexed" => indexed,
      "completed_indexed_degraded" => degraded,
      "completed_skipped" => skipped,
      "completed_deleted" => Map.get(counts, "deleted", 0),
      "completed_oversized" => oversized,
      "graph_ready" => graph_ready,
      "graph_ready_pct" => percentage(graph_ready, total),
      "vector_ready" => vector_ready,
      "vector_ready_file" => vector_ready,
      "semantic_coverage" => vector_ready,
      "semantic_coverage_pct" => percentage(vector_ready, total),
      "vector_ready_file_pct" => percentage(vector_ready, total),
      "vector_ready_file_raw" => decode_single_count(query_rows(global_file_vector_flag_query())),
      "vector_ready_graph" => decode_single_count(query_rows(global_graph_vector_query())),
      "graph_embeddings_count" => decode_single_count(query_rows(global_graph_vector_query())),
      "chunk_embeddings_count" => decode_single_count(query_rows(global_chunk_embedding_query())),
      "vector_ready_graph_pct" =>
        percentage(
          decode_single_count(query_rows(global_graph_vector_query())),
          decode_single_count(query_rows(global_nodes_query()))
        ),
      "nodes_count" => decode_single_count(query_rows(global_nodes_query())),
      "links_count" => decode_single_count(query_rows(global_links_query())),
      "stage_promoted" => Map.get(stage_counts, "promoted", 0),
      "stage_claimed" => Map.get(stage_counts, "claimed", 0),
      "stage_writer_pending_commit" => Map.get(stage_counts, "writer_pending_commit", 0),
      "stage_graph_indexed" => Map.get(stage_counts, "graph_indexed", 0),
      "known" => total,
      "completed" => terminal,
      "indexed_graph_ready" => Map.get(flow_breakdown, "indexed_graph_ready", 0),
      "indexed_graph_missing" => Map.get(flow_breakdown, "indexed_graph_missing", 0),
      "indexed_degraded_graph_ready" => Map.get(flow_breakdown, "indexed_degraded_graph_ready", 0),
      "indexed_degraded_graph_missing" => Map.get(flow_breakdown, "indexed_degraded_graph_missing", 0),
      "indexed_vector_ready" => Map.get(flow_breakdown, "indexed_vector_ready", 0),
      "indexed_vector_missing" => Map.get(flow_breakdown, "indexed_vector_missing", 0),
      "indexed_degraded_vector_ready" => Map.get(flow_breakdown, "indexed_degraded_vector_ready", 0),
      "indexed_degraded_vector_missing" => Map.get(flow_breakdown, "indexed_degraded_vector_missing", 0),
      "soll_done" => 0,
      "soll_partial" => 0,
      "soll_missing" => 0,
      "soll_last_revision" => nil,
      "last_update" => DateTime.utc_now() |> DateTime.to_iso8601()
    }
  end

  def get_directory_stats(repo_code) do
    repo_code
    |> list_projects()
    |> Enum.into(%{}, fn project ->
      {project.project_code,
       %{
         total: project.total,
         completed: project.completed,
         failed: project.degraded + project.oversized,
         last_update: DateTime.utc_now()
       }}
    end)
  end

  def list_projects(_repo_code) do
    project_nodes = query_rows(project_nodes_query()) |> decode_scope_counts()
    project_graph_vectors = query_rows(project_graph_vector_query()) |> decode_scope_counts()
    project_names = query_rows(project_names_query()) |> decode_scope_names()

    readiness_by_project =
      project_ready_query()
      |> query_rows()
      |> Enum.reduce(%{}, fn
        [project_code, graph_ready, vector_ready], acc ->
          Map.put(acc, project_code, %{graph_ready: decode_integer(graph_ready), vector_ready: decode_integer(vector_ready)})

        _row, acc ->
          acc
      end)

    "SELECT COALESCE(project_code, '(unscoped)'), COALESCE(status, 'unknown'), count(*) FROM File GROUP BY 1, 2;"
    |> query_rows()
    |> Enum.group_by(&Enum.at(&1, 0))
    |> Enum.map(fn {project_code, rows} ->
      counts =
        Enum.into(rows, %{}, fn [_project_code, status, count] ->
          {status, decode_integer(count)}
        end)

      total = Enum.sum(Map.values(counts))
      completed = completed_total(counts)
      readiness = Map.get(readiness_by_project, project_code, %{graph_ready: 0, vector_ready: 0})
      project_nodes_count = Map.get(project_nodes, project_code, 0)
      project_graph_vectors_count = Map.get(project_graph_vectors, project_code, 0)

      %{
        project_code: project_code,
        project_name: Map.get(project_names, project_code, project_code),
        display_name: display_project_name(Map.get(project_names, project_code, project_code), project_code),
        known: total,
        total: total,
        completed: completed,
        pending: Map.get(counts, "pending", 0),
        indexing: Map.get(counts, "indexing", 0),
        degraded: Map.get(counts, "indexed_degraded", 0),
        oversized: oversized_total(counts),
        skipped: Map.get(counts, "skipped", 0),
        graph_ready: readiness.graph_ready,
        graph_ready_pct: percentage(readiness.graph_ready, total),
        vector_ready: readiness.vector_ready,
        vector_ready_file: readiness.vector_ready,
        vector_ready_file_pct: percentage(readiness.vector_ready, total),
        vector_ready_graph: project_graph_vectors_count,
        vector_ready_graph_pct: percentage(project_graph_vectors_count, project_nodes_count),
        nodes_count: project_nodes_count,
        links_count: 0,
        progress: percentage(completed, total),
        readiness: readiness_label(counts, total)
      }
    end)
    |> Enum.sort_by(
      fn project ->
        {-project.known, project.project_code}
      end,
      :asc
    )
  end

  def list_backlog_reasons(_repo_code) do
    "SELECT COALESCE(status_reason, 'unknown'), count(*) FROM File WHERE status IN ('pending', 'indexing') GROUP BY 1 ORDER BY 2 DESC LIMIT 8;"
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

        decode_rows(json)

      {:error, reason} ->
        duration_ms = System.monotonic_time(:millisecond) - started_at
        Telemetry.mark_sql_snapshot_error(reason, duration_ms)
        Logger.warning("[cockpit] SQL snapshot query failed after #{duration_ms}ms: #{inspect(reason)}")
        []
    end
  end

  defp decode_rows(payload) when is_binary(payload) do
    case Jason.decode(payload) do
      {:ok, decoded} -> decode_rows(decoded)
      {:error, reason} ->
        Logger.warning("[cockpit] SQL gateway payload invalid JSON: #{inspect(reason)}")
        []
    end
  end

  defp decode_rows(%{"rows" => rows} = envelope) when is_list(rows) do
    columns =
      case envelope do
        %{"columns" => cols} when is_list(cols) -> parse_columns(cols)
        _ -> nil
      end

    rows
    |> Enum.map(&normalize_row(&1, columns))
    |> Enum.filter(&is_list/1)
  end

  defp decode_rows(%{"result" => rows}) when is_list(rows) do
    decode_rows(rows)
  end

  defp decode_rows(%{"data" => rows}) when is_list(rows) do
    decode_rows(rows)
  end

  defp decode_rows([_ | _] = rows) do
    cond do
      Enum.all?(rows, &is_list/1) ->
        rows

      Enum.all?(rows, &is_map/1) ->
        rows

      true ->
        Enum.filter(rows, &(is_list(&1) or is_map(&1)))
    end
  end

  defp decode_rows(_), do: []

  defp parse_columns(columns) when is_list(columns) do
    columns
    |> Enum.map(fn
      col when is_binary(col) -> col
      %{"name" => name} when is_binary(name) -> name
      %{"name" => name} when is_atom(name) -> to_string(name)
      _ -> nil
    end)
    |> Enum.reject(&is_nil/1)
  end

  defp normalize_row(row, nil) when is_list(row), do: row

  defp normalize_row(row, columns) when is_list(columns) and is_list(row) do
    row
  end

  defp normalize_row(row, columns) when is_list(columns) and is_map(row) do
    Enum.map(columns, &Map.get(row, &1))
  end

  defp normalize_row(_row, _columns), do: nil

  defp snapshot_query do
    """
    WITH pending_vector_chunks AS (
      SELECT
        c.file_path AS file_path
      FROM Chunk c
      LEFT JOIN ChunkEmbedding ce
        ON ce.chunk_id = c.id
       AND ce.model_id = '#{active_chunk_model_id()}'
       AND ce.source_hash = c.content_hash
      WHERE ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash
      GROUP BY 1
    ),
    normalized_file AS (
      SELECT
        COALESCE(f.project_code, '(unscoped)') AS project_code,
        COALESCE(f.status, 'unknown') AS status,
        COALESCE(f.file_stage, 'unknown') AS file_stage,
        COALESCE(f.status_reason, 'unknown') AS status_reason,
        f.graph_ready AS graph_ready,
        CASE
          WHEN f.graph_ready = TRUE AND pvc.file_path IS NULL THEN TRUE
          ELSE FALSE
        END AS vector_ready
      FROM File f
      LEFT JOIN pending_vector_chunks pvc ON pvc.file_path = f.path
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
      project_code AS scope,
      status AS key,
      count(*) AS primary_count,
      NULL AS secondary_count
    FROM normalized_file
    GROUP BY 1, 2, 3

    UNION ALL

    SELECT
      'project_ready' AS section,
      project_code AS scope,
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
    WHERE status IN ('pending', 'indexing')
    GROUP BY 1, 2, 3
    """
  end

  defp section_rows(rows, section) do
    Enum.filter(rows, fn
      [^section | _rest] -> true
      %{"section" => ^section} -> true
      %{section: ^section} -> true
      _row -> false
    end)
  end

  defp normalize_counts(rows) do
    Enum.into(rows, %{}, fn
      [status, count] ->
        {status, decode_integer(count)}

      [_section, _scope, status, count, _secondary_count] ->
        {status, decode_integer(count)}

      %{"key" => status, "primary_count" => count} ->
        {status, decode_integer(count)}

      %{key: status, primary_count: count} ->
        {status, decode_integer(count)}

      %{"status" => status} = row ->
        count =
          Map.get(row, "count(*)") ||
            Map.get(row, "count_star()") ||
            Map.get(row, "count") ||
            0

        {status, decode_integer(count)}

      %{status: status} = row ->
        count =
          Map.get(row, :"count(*)") ||
            Map.get(row, :count_star) ||
            Map.get(row, :count) ||
            0

        {status, decode_integer(count)}

      row when is_list(row) and length(row) >= 2 ->
        {Enum.at(row, 0), decode_integer(Enum.at(row, 1))}
    end)
  end

  defp decode_integer(value) when is_integer(value), do: value
  defp decode_integer(value) when is_float(value), do: round(value)

  defp decode_integer(value) when is_binary(value) do
    normalized = String.trim(value)

    cond do
      normalized in ["", "null", "NULL"] ->
        0

      String.starts_with?(normalized, "HugeInt(") and String.ends_with?(normalized, ")") ->
        inner =
          normalized
          |> String.trim_leading("HugeInt(")
          |> String.trim_trailing(")")

        case Integer.parse(inner) do
          {parsed, _} -> parsed
          :error -> 0
        end

      true ->
        case Integer.parse(normalized) do
          {parsed, _} -> parsed
          :error -> 0
        end
    end
  end

  defp decode_integer(_value), do: 0

  defp decode_ready_pair([[graph_ready, vector_ready] | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_ready_pair([%{"graph_ready" => graph_ready, "vector_ready" => vector_ready} | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_ready_pair([%{graph_ready: graph_ready, vector_ready: vector_ready} | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_ready_pair(_rows), do: {0, 0}

  defp decode_snapshot_ready_pair([[_section, _scope, _key, graph_ready, vector_ready] | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_snapshot_ready_pair([%{"primary_count" => graph_ready, "secondary_count" => vector_ready} | _rest]) do
    {decode_integer(graph_ready), decode_integer(vector_ready)}
  end

  defp decode_snapshot_ready_pair([%{primary_count: graph_ready, secondary_count: vector_ready} | _rest]) do
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

  defp workspace_ready_query do
    """
    WITH pending_vector_chunks AS (
      SELECT
        c.file_path AS file_path
      FROM Chunk c
      LEFT JOIN ChunkEmbedding ce
        ON ce.chunk_id = c.id
       AND ce.model_id = '#{active_chunk_model_id()}'
       AND ce.source_hash = c.content_hash
      WHERE ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash
      GROUP BY 1
    )
    SELECT
      SUM(CASE WHEN f.graph_ready THEN 1 ELSE 0 END) AS graph_ready,
      SUM(
        CASE
          WHEN f.graph_ready = TRUE AND pvc.file_path IS NULL THEN 1
          ELSE 0
        END
      ) AS vector_ready
    FROM File f
    LEFT JOIN pending_vector_chunks pvc ON pvc.file_path = f.path
    """
  end

  defp workspace_pipeline_breakdown_query do
    """
    WITH pending_vector_chunks AS (
      SELECT
        c.file_path AS file_path
      FROM Chunk c
      LEFT JOIN ChunkEmbedding ce
        ON ce.chunk_id = c.id
       AND ce.model_id = '#{active_chunk_model_id()}'
       AND ce.source_hash = c.content_hash
      WHERE ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash
      GROUP BY 1
    ),
    normalized_file AS (
      SELECT
        COALESCE(f.status, 'unknown') AS status,
        CASE WHEN f.graph_ready = TRUE THEN TRUE ELSE FALSE END AS graph_ready,
        CASE
          WHEN f.graph_ready = TRUE AND pvc.file_path IS NULL THEN TRUE
          ELSE FALSE
        END AS vector_ready
      FROM File f
      LEFT JOIN pending_vector_chunks pvc ON pvc.file_path = f.path
    )
    SELECT
      SUM(CASE WHEN status = 'indexed' AND graph_ready THEN 1 ELSE 0 END) AS indexed_graph_ready,
      SUM(CASE WHEN status = 'indexed' AND NOT graph_ready THEN 1 ELSE 0 END) AS indexed_graph_missing,
      SUM(CASE WHEN status = 'indexed_degraded' AND graph_ready THEN 1 ELSE 0 END) AS indexed_degraded_graph_ready,
      SUM(CASE WHEN status = 'indexed_degraded' AND NOT graph_ready THEN 1 ELSE 0 END) AS indexed_degraded_graph_missing,
      SUM(CASE WHEN status = 'indexed' AND vector_ready THEN 1 ELSE 0 END) AS indexed_vector_ready,
      SUM(CASE WHEN status = 'indexed' AND graph_ready AND NOT vector_ready THEN 1 ELSE 0 END) AS indexed_vector_missing,
      SUM(CASE WHEN status = 'indexed_degraded' AND vector_ready THEN 1 ELSE 0 END) AS indexed_degraded_vector_ready,
      SUM(CASE WHEN status = 'indexed_degraded' AND graph_ready AND NOT vector_ready THEN 1 ELSE 0 END) AS indexed_degraded_vector_missing
    FROM normalized_file
    """
  end

  defp project_ready_query do
    """
    WITH pending_vector_chunks AS (
      SELECT
        c.file_path AS file_path
      FROM Chunk c
      LEFT JOIN ChunkEmbedding ce
        ON ce.chunk_id = c.id
       AND ce.model_id = '#{active_chunk_model_id()}'
       AND ce.source_hash = c.content_hash
      WHERE ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash
      GROUP BY 1
    )
    SELECT
      COALESCE(f.project_code, '(unscoped)') AS project_code,
      SUM(CASE WHEN f.graph_ready THEN 1 ELSE 0 END) AS graph_ready,
      SUM(
        CASE
          WHEN f.graph_ready = TRUE AND pvc.file_path IS NULL THEN 1
          ELSE 0
        END
      ) AS vector_ready
    FROM File f
    LEFT JOIN pending_vector_chunks pvc ON pvc.file_path = f.path
    GROUP BY 1
    """
  end

  defp global_graph_vector_query do
    "SELECT COUNT(DISTINCT anchor_type || ':' || anchor_id) FROM GraphEmbedding"
  end

  defp global_chunk_embedding_query do
    "SELECT COUNT(*) FROM ChunkEmbedding WHERE model_id = '#{active_chunk_model_id()}'"
  end

  defp global_file_vector_flag_query do
    "SELECT COUNT(*) FROM File WHERE vector_ready = TRUE"
  end

  defp active_chunk_model_id do
    System.get_env("AXON_CHUNK_MODEL_ID")
    |> case do
      nil -> @default_chunk_model_id
      value ->
        normalized = String.trim(value)
        if normalized == "", do: @default_chunk_model_id, else: normalized
    end
  end

  defp global_nodes_query do
    "SELECT COUNT(*) FROM Symbol"
  end

  defp global_links_query do
    """
    SELECT (
      COALESCE((SELECT COUNT(*) FROM CALLS), 0) +
      COALESCE((SELECT COUNT(*) FROM CONTAINS), 0) +
      COALESCE((SELECT COUNT(*) FROM IMPACTS), 0) +
      COALESCE((SELECT COUNT(*) FROM SUBSTANTIATES), 0)
    ) AS links_count
    """
  end

  defp project_nodes_query do
    "SELECT COALESCE(project_code, '(unscoped)'), COUNT(*) FROM Symbol GROUP BY 1"
  end

  defp project_links_query do
    """
    WITH edge_sources AS (
      SELECT source_id FROM CALLS
      UNION ALL
      SELECT source_id FROM CONTAINS
      UNION ALL
      SELECT source_id FROM IMPACTS
      UNION ALL
      SELECT source_id FROM SUBSTANTIATES
    )
    SELECT COALESCE(s.project_code, '(unscoped)'), COUNT(*)
    FROM edge_sources e
    LEFT JOIN Symbol s ON s.id = e.source_id
    GROUP BY 1
    """
  end

  defp project_graph_vector_query do
    """
    SELECT COALESCE(s.project_code, '(unscoped)'), COUNT(DISTINCT g.anchor_type || ':' || g.anchor_id)
    FROM GraphEmbedding g
    LEFT JOIN Symbol s ON g.anchor_type = 'Symbol' AND s.id = g.anchor_id
    GROUP BY 1
    """
  end

  defp project_names_query do
    """
    SELECT
      COALESCE(project_code, '(unscoped)'),
      COALESCE(NULLIF(project_name, ''), project_code)
    FROM soll.ProjectCodeRegistry
    """
  end

  defp decode_single_count([[value | _] | _]), do: decode_integer(value)
  defp decode_single_count([%{"count" => value} | _]), do: decode_integer(value)
  defp decode_single_count(_), do: 0

  defp decode_scope_counts(rows) do
    Enum.reduce(rows, %{}, fn
      [scope, value | _], acc when is_binary(scope) ->
        Map.put(acc, scope, decode_integer(value))

      _row, acc ->
        acc
    end)
  end

  defp decode_scope_names(rows) do
    Enum.reduce(rows, %{}, fn
      [scope, value | _], acc when is_binary(scope) ->
        Map.put(acc, scope, normalize_project_name(value, scope))

      _row, acc ->
        acc
    end)
  end

  defp decode_flow_breakdown([[a, b, c, d, e, f, g, h | _rest]]) do
    %{
      "indexed_graph_ready" => decode_integer(a),
      "indexed_graph_missing" => decode_integer(b),
      "indexed_degraded_graph_ready" => decode_integer(c),
      "indexed_degraded_graph_missing" => decode_integer(d),
      "indexed_vector_ready" => decode_integer(e),
      "indexed_vector_missing" => decode_integer(f),
      "indexed_degraded_vector_ready" => decode_integer(g),
      "indexed_degraded_vector_missing" => decode_integer(h)
    }
  end

  defp decode_flow_breakdown(
         [
           %{
             "indexed_graph_ready" => a,
             "indexed_graph_missing" => b,
             "indexed_degraded_graph_ready" => c,
             "indexed_degraded_graph_missing" => d,
             "indexed_vector_ready" => e,
             "indexed_vector_missing" => f,
             "indexed_degraded_vector_ready" => g,
             "indexed_degraded_vector_missing" => h
           }
           | _rest
         ]
       ) do
    %{
      "indexed_graph_ready" => decode_integer(a),
      "indexed_graph_missing" => decode_integer(b),
      "indexed_degraded_graph_ready" => decode_integer(c),
      "indexed_degraded_graph_missing" => decode_integer(d),
      "indexed_vector_ready" => decode_integer(e),
      "indexed_vector_missing" => decode_integer(f),
      "indexed_degraded_vector_ready" => decode_integer(g),
      "indexed_degraded_vector_missing" => decode_integer(h)
    }
  end

  defp decode_flow_breakdown(_rows) do
    %{
      "indexed_graph_ready" => 0,
      "indexed_graph_missing" => 0,
      "indexed_degraded_graph_ready" => 0,
      "indexed_degraded_graph_missing" => 0,
      "indexed_vector_ready" => 0,
      "indexed_vector_missing" => 0,
      "indexed_degraded_vector_ready" => 0,
      "indexed_degraded_vector_missing" => 0
    }
  end

  defp normalize_project_name(value, fallback) when is_binary(value) do
    case String.trim(value) do
      "" -> fallback
      trimmed -> trimmed
    end
  end

  defp normalize_project_name(_value, fallback), do: fallback

  defp display_project_name(project_name, project_code) do
    normalized_name = normalize_project_name(project_name, project_code)

    if normalized_name == project_code do
      project_code
    else
      "#{normalized_name} (#{project_code})"
    end
  end

  defp soll_coverage_query do
    """
    SELECT
      COUNT(*) AS total_requirements,
      SUM(
        CASE
          WHEN COALESCE(r.acceptance_criteria, '') NOT IN ('', '[]')
               AND COALESCE(r.status, '') IN ('current', 'accepted')
               AND EXISTS (
                 SELECT 1
                 FROM soll.Traceability t
                 WHERE t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
               )
          THEN 1 ELSE 0
        END
      ) AS done_requirements,
      SUM(
        CASE
          WHEN (
            COALESCE(r.acceptance_criteria, '') NOT IN ('', '[]')
            OR EXISTS (
              SELECT 1
              FROM soll.Traceability t
              WHERE t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
            )
          )
          AND NOT (
            COALESCE(r.acceptance_criteria, '') NOT IN ('', '[]')
            AND COALESCE(r.status, '') IN ('current', 'accepted')
            AND EXISTS (
              SELECT 1
              FROM soll.Traceability t
              WHERE t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
            )
          )
          THEN 1 ELSE 0
        END
      ) AS partial_requirements,
      SUM(
        CASE
          WHEN COALESCE(r.acceptance_criteria, '') IN ('', '[]')
               AND NOT EXISTS (
                 SELECT 1
                 FROM soll.Traceability t
                 WHERE t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
               )
          THEN 1 ELSE 0
        END
      ) AS missing_requirements
    FROM soll.Requirement r
    """
  end

  defp soll_revision_query do
    "SELECT revision_id FROM soll.Revision ORDER BY committed_at DESC LIMIT 1"
  end

  defp decode_soll_metric([row | _], idx) when is_list(row) do
    row |> Enum.at(idx) |> decode_integer()
  end

  defp decode_soll_metric(_rows, _idx), do: 0

  defp decode_soll_revision([[revision_id | _] | _]) when is_binary(revision_id), do: revision_id
  defp decode_soll_revision(_), do: nil
end
