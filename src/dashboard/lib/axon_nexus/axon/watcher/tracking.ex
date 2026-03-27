defmodule Axon.Watcher.Tracking do
  @moduledoc """
  The Tracking context.
  """

  import Ecto.Query, warn: false
  alias Axon.Watcher.Repo
  alias Axon.Watcher.IndexedProject
  alias Axon.Watcher.IndexedFile

  def update_project_scores(project, security_score, coverage_score) do
    project
    |> Ecto.Changeset.change(%{security_score: security_score, coverage_score: coverage_score})
    |> Repo.update()
  end

  @doc """
  Inserts the project if it doesn't exist.
  """
  def upsert_project!(name, path, status \\ "active") do
    id = name
    attrs = %{id: id, name: name, path: path, status: status}

    case Repo.get(IndexedProject, id) do
      nil ->
        %IndexedProject{}
        |> IndexedProject.changeset(attrs)
        |> Repo.insert!()

      project ->
        project
        |> IndexedProject.changeset(attrs)
        |> Repo.update!()
    end
  end

  @doc """
  Inserts or updates the file. (Unit wrapper for compatibility)
  """
  def upsert_file!(project_id, path, file_hash, status \\ "pending") do
    upsert_files_batch!(project_id, [{path, file_hash, status}])
  end

  @doc """
  Inserts or updates multiple files in a single transaction (Standard version).
  """
  def upsert_files_batch!(project_id, file_data_list) do
    now = DateTime.utc_now() |> DateTime.truncate(:second)
    
    entries = Enum.map(file_data_list, fn {path, file_hash, status} ->
      %{
        id: path,
        project_id: project_id,
        path: path,
        file_hash: file_hash,
        status: status,
        inserted_at: now,
        updated_at: now
      }
    end)

    Repo.insert_all(IndexedFile, entries, 
      on_conflict: {:replace, [:file_hash, :status, :updated_at]},
      conflict_target: :id
    )
  end

  @doc """
  Inserts or updates multiple files with full metrics in a single transaction.
  """
  def upsert_files_full_batch!(project_id, file_data_list) do
    # file_data_list: [{path, hash, status, symbols, relations, sec, cov, duration, ram_b, ram_a}, ...]
    now = DateTime.utc_now() |> DateTime.truncate(:second)
    
    entries = Enum.map(file_data_list, fn {path, hash, status, syms, rels, sec, cov, dur, rb, ra} ->
      %{
        id: path,
        project_id: project_id,
        path: path,
        file_hash: hash,
        status: status,
        symbols_count: syms,
        relations_count: rels,
        security_score: sec,
        coverage_score: cov,
        ingestion_duration_ms: dur,
        ram_before_mb: rb,
        ram_after_mb: ra,
        inserted_at: now,
        updated_at: now
      }
    end)

    Repo.insert_all(IndexedFile, entries, 
      on_conflict: {:replace, [:file_hash, :status, :symbols_count, :relations_count, :security_score, :coverage_score, :ingestion_duration_ms, :ram_before_mb, :ram_after_mb, :updated_at]},
      conflict_target: :id
    )
  end

  @doc """
  Updates status for multiple files in a single transaction.
  """
  def mark_files_status_batch!(status_map) do
    # status_map: %{path => %{status: "ok", symbols_count: 10, ...}}
    Repo.transaction(fn ->
      Enum.each(status_map, fn {path, params} ->
        mark_file_status!(path, params.status, Map.delete(params, :status))
      end)
    end)
  end

  @doc """
  Updates the file with the given status and optional params.
  """
  def mark_file_status!(path, status, params \\ %{}) do
    case Repo.get(IndexedFile, path) do
      nil ->
        raise "File not found: #{path}"

      file ->
        attrs = Map.put(params, :status, status)

        file
        |> IndexedFile.changeset(attrs)
        |> Repo.update!()
    end
  end

  @doc """
  Gets the current status of a file.
  """
  def get_file_status(path) do
    case Repo.get(IndexedFile, path) do
      nil -> nil
      file -> file.status
    end
  end

  @doc """
  Gets the current file hash (mtime) of a file.
  """
  def get_file_hash(path) do
    case Repo.get(IndexedFile, path) do
      nil -> nil
      file -> file.file_hash
    end
  end

  @doc """
  Guesses the project ID (top-level dir name) from a file path.
  """
  def extract_project_from_path(path) do
    # Assuming standard structure /home/dstadel/projects/PROJECT_NAME/...
    case Path.split(path) do
      [_, _, _, _, project | _] -> project
      _ -> "global"
    end
  end

  @doc """
  Gets the project ID of a file.
  """
  def get_project_for_file(path) do
    query =
      from(f in IndexedFile,
        join: p in IndexedProject,
        on: f.project_id == p.id,
        where: f.path == ^path,
        select: p.id
      )

    Repo.one(query)
  end

  @doc """
  Gets up to `limit` files that are currently in 'failed' state.
  """
  def get_failed_files(limit \\ 100) do
    query =
      from(f in IndexedFile,
        where: f.status == "failed",
        limit: ^limit,
        select: f.path
      )

    Repo.all(query)
  end

  @doc """
  Returns a map of projects and their file statistics, and the top 14 recently updated files.
  """
  def get_dashboard_stats() do
    directories = get_directory_stats()
    last_files = get_recent_files(14)

    %{
      directories: directories,
      last_files: last_files
    }
  end

  defp get_directory_stats() do
    query =
      from(p in IndexedProject,
        left_join: f in IndexedFile,
        on: f.project_id == p.id,
        group_by: p.name,
        select: {
          p.name,
          count(f.id),
          fragment("coalesce(sum(case when ? = 'indexed' then 1 else 0 end), 0)", f.status),
          fragment("coalesce(sum(case when ? = 'failed' then 1 else 0 end), 0)", f.status),
          fragment(
            "coalesce(sum(case when ? = 'ignored_by_rule' then 1 else 0 end), 0)",
            f.status
          ),
          fragment("coalesce(sum(?), 0)", f.symbols_count),
          fragment("coalesce(sum(?), 0)", f.relations_count),
          fragment("coalesce(sum(?), 0)", f.is_entry_point),
          fragment(
            "coalesce(avg(case when ? = 'indexed' then ? else null end), 100)",
            f.status,
            f.security_score
          ),
          fragment(
            "coalesce(avg(case when ? = 'indexed' then ? else null end), 0)",
            f.status,
            f.coverage_score
          )
        }
      )

    Repo.all(query)
    |> Enum.reduce(%{}, fn {project_name, total, completed, failed, ignored, syms, rels, entries,
                            sec, cov},
                           acc ->
      Map.put(acc, project_name, %{
        total: total || 0,
        completed: completed || 0,
        failed: failed || 0,
        ignored: ignored || 0,
        symbols: syms || 0,
        relations: rels || 0,
        entries: entries || 0,
        security: round(sec || 100),
        coverage: round(cov || 0)
      })
    end)
  end

  defp get_recent_files(limit) do
    query =
      from(f in IndexedFile,
        order_by: [desc: f.updated_at],
        limit: ^limit,
        select: %{
          path: f.path,
          status: f.status,
          time: f.updated_at
        }
      )

    Repo.all(query)
  end
end
