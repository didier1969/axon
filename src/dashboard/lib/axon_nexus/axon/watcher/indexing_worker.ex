defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing_default, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}, id: job_id}) do
    Logger.info("[Oban] Processing batch of #{length(batch)} files (Job #{job_id})")
    
    Enum.each(batch, fn file ->
      Axon.Watcher.Telemetry.report_start("oban:#{job_id}", file["path"])
      
      case PoolFacade.parse(file["path"], file["content"]) do
        %{"status" => "ok"} ->
          Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", file["path"], :ok)
          Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:file_indexed, file["path"], :ok})
        error ->
          Logger.error("[Oban] Failed to parse #{file["path"]}: #{inspect(error)}")
          Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", file["path"], {:error, error})
          Phoenix.PubSub.broadcast(AxonDashboard.PubSub, "bridge_events", {:file_indexed, file["path"], :error})
      end
    end)
    :ok
  end
end
