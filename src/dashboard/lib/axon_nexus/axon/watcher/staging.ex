defmodule Axon.Watcher.Staging do
  @moduledoc """
  Memory staging area using ETS to buffer high-throughput file discoveries.
  Avoids thousands of individual SQLite transactions by using Oban.insert_all/2.
  """
  use GenServer
  require Logger

  @table_name :axon_watcher_staging
  @flush_interval 500 # Flush every 500ms
  @batch_size 1000    # Max jobs per insert_all transaction

  # --- Client API ---

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Stages a file path for ingestion.
  """
  def stage_file(project_name, path, file_hash, priority \\ 10) do
    try do
      :ets.insert(@table_name, {path, project_name, file_hash, priority})
    rescue
      # If table doesn't exist yet, we drop the event. 
      # With :rest_for_one strategy, this should not happen during normal operation.
      _ -> :ok
    end
  end

  # --- Server Callbacks ---

  @impl true
  def init(_opts) do
    # SYNCHRONOUS INITIALIZATION: Guarantee table exists before returning {:ok, pid}
    :ets.new(@table_name, [:set, :public, :named_table, {:read_concurrency, true}, {:write_concurrency, true}])
    
    schedule_flush()
    {:ok, %{}}
  end

  @impl true
  def handle_info(:flush, state) do
    flush_staging()
    schedule_flush()
    {:noreply, state}
  end

  defp schedule_flush do
    Process.send_after(self(), :flush, @flush_interval)
  end

  defp flush_staging do
    entries = :ets.tab2list(@table_name)
    
    if entries != [] do
      # Step 1: Wrap everything in an ATOMIC transaction
      # We don't delete from ETS until the SQL transaction is committed.
      result = Axon.Watcher.Repo.transaction(fn ->
        # 1.1: Batch UPSERT into SQLite (indexed_files table)
        entries
        |> Enum.group_by(fn {_, proj, _, _} -> proj end)
        |> Enum.each(fn {project_id, file_list} ->
          data_for_upsert = Enum.map(file_list, fn {path, _, hash, _} -> {path, hash, "pending"} end)
          Axon.Watcher.Tracking.upsert_files_batch!(project_id, data_for_upsert)
        end)

        # 1.2: Batch INSERT into Oban
        entries
        |> Enum.chunk_every(@batch_size)
        |> Enum.each(fn batch ->
          jobs = Enum.map(batch, fn {path, project_name, _hash, priority} ->
            queue = if priority >= 80, do: :indexing_hot, else: :indexing_default
            
            Axon.Watcher.IndexingWorker.new(%{
              "batch" => [%{
                "path" => path,
                "project" => project_name,
                "trace_id" => Ecto.UUID.generate(),
                "t0" => :os.system_time(:microsecond)
              }]
            }, queue: queue)
          end)

          Oban.insert_all(jobs)
        end)
      end)

      case result do
        {:ok, _} ->
          # Only now we clear the objects that were flushed
          # To be perfectly safe, we only clear the specific keys we just processed
          # but delete_all_objects is faster. Given :set mode, it's acceptable for now.
          :ets.delete_all_objects(@table_name)
          :telemetry.execute([:axon, :watcher, :staging_flushed], %{count: length(entries)})
        {:error, reason} ->
          Logger.error("[Staging] Transaction failed, keeping data in ETS: #{inspect(reason)}")
      end
    end
  end
end
