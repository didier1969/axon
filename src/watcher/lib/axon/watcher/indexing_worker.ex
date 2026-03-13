defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing_default, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}, id: job_id}) do
    Enum.each(batch, fn file ->
      Axon.Watcher.Telemetry.report_start("oban:#{job_id}", file["path"])
      
      case PoolFacade.parse(file["path"], file["content"]) do
        %{"status" => "ok"} ->
          Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", file["path"], :ok)
        error ->
          Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", file["path"], {:error, error})
      end
    end)
    :ok
  end
end
