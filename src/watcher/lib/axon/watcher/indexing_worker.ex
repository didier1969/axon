defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}}) do
    case PoolFacade.parse_batch(batch) do
      %{"status" => "ok", "data" => data} ->
        Logger.info("[Oban] Successfully parsed #{length(data)} files. Ingesting to HydraDB...")
        # L'ingestion se fait dans PoolFacade ou ici
        # Pour l'instant on se fie au succès du parsing
        :ok

      error ->
        Logger.error("[Oban] Batch failed during parsing: #{inspect(error)}")
        {:error, error}
    end
  end
end
