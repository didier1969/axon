defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade
  alias Axon.Watcher.Schemas.ExtractionResult

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}}) do
    case PoolFacade.parse_batch(batch) do
      %{"status" => "ok", "data" => data} ->
        # Validation rigide avec Ecto
        valid_results = Enum.reduce(data, [], fn item, acc ->
          changeset = ExtractionResult.changeset(%ExtractionResult{}, item)
          if changeset.valid? do
            [Ecto.Changeset.apply_changes(changeset) | acc]
          else
            Logger.warning("[Oban] Invalid data for #{item["path"]}: #{inspect(changeset.errors)}")
            acc
          end
        end)

        if length(valid_results) > 0 do
          Logger.info("[Oban] Persistent ingestion of #{length(valid_results)} files.")
          # Ici on pourrait notifier HydraDB du succès de ce batch spécifique
          :ok
        else
          {:error, :validation_failed}
        end

      error ->
        Logger.error("[Oban] Batch failed: #{inspect(error)}")
        {:error, error}
    end
  end
end
