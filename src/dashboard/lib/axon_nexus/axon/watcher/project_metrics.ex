defmodule Axon.Watcher.ProjectMetrics do
  @moduledoc """
  Per-project quality + indexing-rate metrics for the dashboard /projects page.

  Queries PG live via the existing `Axon.Watcher.SqlGateway` HTTP /sql
  endpoint. All queries return columns by positional index because the
  gateway's JSON response is a list-of-lists (`[[col1, col2, ...], ...]`).

  Layout of the table shown in the UI:

    project_code | files | chunks | embedded | coverage% | symbols | edges | last_activity_ms | rate_per_min

  `last_activity_ms` = max(embedded_at_ms) of the project, falling back
  to max(last_seen_ms) when no embeddings yet.

  `rate_per_min` = count(chunkembedding) in the last 60s for that project.
  """

  alias Axon.Watcher.SqlGateway

  @doc """
  Fetch all projects with their aggregate metrics. Returns a list of maps:

      [%{project_code, files, chunks, embedded, coverage_pct, symbols, edges,
         last_activity_ms, rate_per_min, indexing_active?}, ...]
  """
  def fetch_all do
    %{
      chunks_by_project: chunks_by_project(),
      embedded_by_project: embedded_by_project(),
      symbols_by_project: symbols_by_project(),
      edges_by_project: edges_by_project(),
      last_activity_by_project: last_activity_by_project(),
      rate_by_project: recent_rate_by_project(60_000)
    }
    |> merge_projects()
  end

  defp merge_projects(maps) do
    project_codes =
      maps
      |> Map.values()
      |> Enum.flat_map(&Map.keys/1)
      |> Enum.uniq()

    now_ms = System.system_time(:millisecond)

    project_codes
    |> Enum.map(fn pc ->
      chunks = Map.get(maps.chunks_by_project, pc, 0)
      embedded = Map.get(maps.embedded_by_project, pc, 0)
      symbols = Map.get(maps.symbols_by_project, pc, 0)
      edges = Map.get(maps.edges_by_project, pc, 0)
      last_activity_ms = Map.get(maps.last_activity_by_project, pc, 0)
      rate_per_min = Map.get(maps.rate_by_project, pc, 0)

      coverage_pct =
        if chunks > 0, do: Float.round(embedded * 100.0 / chunks, 2), else: 0.0

      age_ms = if last_activity_ms > 0, do: max(0, now_ms - last_activity_ms), else: nil

      %{
        project_code: pc,
        files: 0,
        chunks: chunks,
        embedded: embedded,
        coverage_pct: coverage_pct,
        symbols: symbols,
        edges: edges,
        last_activity_ms: last_activity_ms,
        age_ms: age_ms,
        rate_per_min: rate_per_min,
        indexing_active?: rate_per_min > 0,
        pending: max(0, chunks - embedded)
      }
    end)
    |> Enum.sort_by(& &1.chunks, :desc)
  end

  ## Individual queries

  defp chunks_by_project do
    fetch_rows("SELECT project_code, count(*) AS n FROM public.chunk GROUP BY 1")
    |> Enum.into(%{}, fn [pc, n] -> {pc, to_int(n)} end)
  end

  defp embedded_by_project do
    fetch_rows("SELECT project_code, count(*) AS n FROM public.chunkembedding GROUP BY 1")
    |> Enum.into(%{}, fn [pc, n] -> {pc, to_int(n)} end)
  end

  defp symbols_by_project do
    fetch_rows("SELECT project_code, count(*) AS n FROM public.symbol GROUP BY 1")
    |> Enum.into(%{}, fn [pc, n] -> {pc, to_int(n)} end)
  end

  defp edges_by_project do
    fetch_rows("SELECT project_code, count(*) AS n FROM public.edge GROUP BY 1")
    |> Enum.into(%{}, fn [pc, n] -> {pc, to_int(n)} end)
  end

  defp last_activity_by_project do
    fetch_rows(
      "SELECT project_code, max(embedded_at_ms) AS last_ms FROM public.chunkembedding GROUP BY 1"
    )
    |> Enum.into(%{}, fn [pc, n] -> {pc, to_int(n)} end)
  end

  defp recent_rate_by_project(window_ms) do
    cutoff = System.system_time(:millisecond) - window_ms

    fetch_rows(
      "SELECT project_code, count(*) AS n FROM public.chunkembedding " <>
        "WHERE embedded_at_ms > #{cutoff} GROUP BY 1"
    )
    |> Enum.into(%{}, fn [pc, n] -> {pc, to_int(n)} end)
  end

  ## Helpers

  defp fetch_rows(sql) do
    case SqlGateway.query_json(sql) do
      {:ok, body} ->
        case Jason.decode(body) do
          {:ok, rows} when is_list(rows) -> rows
          _ -> []
        end

      _ ->
        []
    end
  end

  defp to_int(n) when is_integer(n), do: n
  defp to_int(n) when is_binary(n) do
    case Integer.parse(n) do
      {i, _} -> i
      _ -> 0
    end
  end
  defp to_int(n) when is_float(n), do: trunc(n)
  defp to_int(_), do: 0

  @doc """
  Global totals across all projects (matches `embedding_status` rollup).
  """
  def fetch_totals do
    %{
      indexed_files: fetch_scalar("SELECT count(*) FROM public.indexedfile"),
      chunks: fetch_scalar("SELECT count(*) FROM public.chunk"),
      embedded: fetch_scalar("SELECT count(*) FROM public.chunkembedding"),
      symbols: fetch_scalar("SELECT count(*) FROM public.symbol"),
      edges: fetch_scalar("SELECT count(*) FROM public.edge")
    }
  end

  defp fetch_scalar(sql) do
    case fetch_rows(sql) do
      [[n] | _] -> to_int(n)
      _ -> 0
    end
  end
end
