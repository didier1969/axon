defmodule Axon.Watcher.Tracking do
  @moduledoc """
  The Tracking context.
  """

  import Ecto.Query, warn: false
  alias Axon.Watcher.Repo
  alias Axon.Watcher.IndexedProject
  alias Axon.Watcher.IndexedFile

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
  Inserts or updates the file.
  """
  def upsert_file!(project_id, path, file_hash, status \\ "pending") do
    id = path
    attrs = %{id: id, project_id: project_id, path: path, file_hash: file_hash, status: status}

    case Repo.get(IndexedFile, id) do
      nil ->
        %IndexedFile{}
        |> IndexedFile.changeset(attrs)
        |> Repo.insert!()

      file ->
        file
        |> IndexedFile.changeset(attrs)
        |> Repo.update!()
    end
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
      from p in IndexedProject,
        left_join: f in IndexedFile,
        on: f.project_id == p.id,
        group_by: p.name,
        select: {
          p.name,
          count(f.id),
          fragment("coalesce(sum(case when ? = 'indexed' then 1 else 0 end), 0)", f.status),
          fragment("coalesce(sum(case when ? = 'failed' then 1 else 0 end), 0)", f.status),
          fragment("coalesce(sum(case when ? = 'ignored_by_rule' then 1 else 0 end), 0)", f.status)
        }

    Repo.all(query)
    |> Enum.reduce(%{}, fn {project_name, total, completed, failed, ignored}, acc ->
      Map.put(acc, project_name, %{
        total: total || 0,
        completed: completed || 0,
        failed: failed || 0,
        ignored: ignored || 0
      })
    end)
  end

  defp get_recent_files(limit) do
    query =
      from f in IndexedFile,
        order_by: [desc: f.updated_at],
        limit: ^limit,
        select: %{
          path: f.path,
          status: f.status,
          time: f.updated_at
        }

    Repo.all(query)
  end
end
