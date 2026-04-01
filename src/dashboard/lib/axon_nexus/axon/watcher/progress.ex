defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress using DuckDB as the sole source of truth.
  v6.0 Consolidation - Replaces SQLite and HydraDB reporting.
  """
  require Logger
  alias Axon.Watcher.SqlGateway

  def get_status(_repo_slug) do
    query = "SELECT status, count(*) as count FROM File GROUP BY status;"

    db_status =
      case SqlGateway.query_json(query) do
        {:ok, json} ->
          case Jason.decode(json) do
          {:ok, rows} ->
            stats =
              Enum.into(rows, %{}, fn [status, count] ->
                parsed_count =
                  case count do
                    n when is_integer(n) -> n
                    n when is_float(n) -> round(n)
                    n when is_binary(n) ->
                      case Integer.parse(n) do
                        {value, _} -> value
                        :error -> 0
                      end

                    _ ->
                      0
                  end

                {status, parsed_count}
              end)
              total = Enum.sum(Map.values(stats))
              indexed =
                Map.get(stats, "indexed", 0) + Map.get(stats, "indexed_degraded", 0)

              progress = if total > 0, do: round((indexed / total) * 100), else: 0

              %{
                "status" => "live",
                "progress" => progress,
                "synced" => indexed,
                "total" => total,
                "last_update" => DateTime.utc_now() |> DateTime.to_iso8601()
              }

            _ ->
              default_status()
          end

        _ ->
          default_status()
      end

    db_status
  end

  def get_directory_stats(_repo_slug) do
    # On agrège les stats par projet directement depuis DuckDB
    query = "SELECT project_slug, status, count(*) as count FROM File GROUP BY project_slug, status;"

    case SqlGateway.query_json(query) do
      {:ok, json} ->
        case Jason.decode(json) do
          {:ok, rows} ->
            rows
            |> Enum.group_by(fn [slug, _status, _count] -> slug end)
            |> Enum.into(%{}, fn {slug, project_rows} ->
              total = Enum.sum(Enum.map(project_rows, fn [_, _, c] -> c end))
              completed =
                Enum.reduce(project_rows, 0, fn
                  [_, status, c], acc when status in ["indexed", "indexed_degraded"] -> acc + c
                  _, acc -> acc
                end)

              {slug,
               %{
                 total: total,
                 completed: completed,
                 failed: 0,
                 last_update: DateTime.utc_now()
               }}
            end)

          _ ->
            %{}
        end

      _ ->
        %{}
    end
  end

  defp default_status do
    %{
      "status" => "connecting",
      "progress" => 0,
      "synced" => 0,
      "total" => 0,
      "last_update" => DateTime.utc_now() |> DateTime.to_iso8601()
    }
  end

  def get_file_mtime(_, _), do: 0
  def save_file_mtime(_, _, _), do: :ok
end
