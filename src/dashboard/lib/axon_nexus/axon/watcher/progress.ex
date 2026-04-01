defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress using DuckDB as the sole source of truth.
  v6.0 Consolidation - Replaces SQLite and HydraDB reporting.
  """
  require Logger
  alias Axon.Watcher.SqlGateway

  @overlay_prefix {:axon, :watcher, :progress}

  def get_status(repo_slug) do
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
              indexed = Map.get(stats, "indexed", 0)

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

    merge_overlay_status(repo_slug, db_status)
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
                Enum.find_value(project_rows, 0, fn
                  [_, "indexed", c] -> c
                  _ -> false
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

  def update_status(repo_slug, attrs) when is_binary(repo_slug) and is_map(attrs) do
    previous = read_overlay(repo_slug)

    merged =
      previous
      |> Map.merge(stringify_keys(attrs))
      |> Map.put("last_update", DateTime.utc_now() |> DateTime.to_iso8601())

    :persistent_term.put(overlay_key(repo_slug), merged)
    :ok
  end

  def purge_repo(repo_slug) when is_binary(repo_slug) do
    :persistent_term.erase(overlay_key(repo_slug))
    :ok
  end

  def get_file_mtime(_, _), do: 0
  def save_file_mtime(_, _, _), do: :ok

  defp merge_overlay_status(repo_slug, db_status) do
    overlay = read_overlay(repo_slug)

    case overlay do
      %{} = overlay when map_size(overlay) > 0 ->
        Map.merge(db_status, overlay)

      _ ->
        db_status
    end
  end

  defp read_overlay(repo_slug) do
    :persistent_term.get(overlay_key(repo_slug), %{})
  end

  defp overlay_key(repo_slug), do: {@overlay_prefix, repo_slug}

  defp stringify_keys(map) do
    Enum.into(map, %{}, fn {key, value} -> {to_string(key), value} end)
  end
end
