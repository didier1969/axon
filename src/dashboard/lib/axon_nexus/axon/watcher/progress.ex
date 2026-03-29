defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress using DuckDB as the sole source of truth.
  v6.0 Consolidation - Replaces SQLite and HydraDB reporting.
  """
  require Logger
  alias Axon.Watcher.PoolFacade

  def get_status(_repo_slug) do
    query = "SELECT status, count(*) as count FROM File GROUP BY status;"
    
    case PoolFacade.query_json(query) do
      {:ok, json} ->
        case Jason.decode(json) do
          {:ok, rows} ->
            stats = Enum.into(rows, %{}, fn [status, count] -> {status, count} end)
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
  end

  def get_directory_stats(_repo_slug) do
    # On agrège les stats par projet directement depuis DuckDB
    query = "SELECT project_slug, status, count(*) as count FROM File GROUP BY project_slug, status;"
    
    case PoolFacade.query_json(query) do
      {:ok, json} ->
        case Jason.decode(json) do
          {:ok, rows} ->
            rows
            |> Enum.group_by(fn [slug, _status, _count] -> slug end)
            |> Enum.into(%{}, fn {slug, project_rows} ->
              total = Enum.sum(Enum.map(project_rows, fn [_, _, c] -> c end))
              completed = Enum.find_value(project_rows, 0, fn 
                [_, "indexed", c] -> c
                _ -> false 
              end)
              
              {slug, %{
                total: total,
                completed: completed,
                failed: 0,
                last_update: DateTime.utc_now()
              }}
            end)
          _ -> %{}
        end
      _ -> %{}
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

  # Obsolete methods kept for compatibility with other modules but neutralized
  def update_status(_, _), do: :ok
  def purge_repo(_), do: :ok
  def get_file_mtime(_, _), do: 0
  def save_file_mtime(_, _, _), do: :ok
end
