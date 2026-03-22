defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing_default, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}, id: job_id}) do
    Logger.info("[Oban] Processing batch of #{length(batch)} files (Job #{job_id})")
    
    Enum.each(batch, fn file ->
      path = file["path"]
      Axon.Watcher.Telemetry.report_start("oban:#{job_id}", path)
      
      case File.read(path) do
        {:ok, content} ->
          if String.printable?(content) do
            case PoolFacade.parse(path, content) do
              %{"status" => "ok"} ->
                Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, :ok)
                PoolFacade.broadcast_event("WatcherFileIndexed", %{path: path, status: "ok"})
              error ->
                Logger.error("[Oban] Failed to parse #{path}: #{inspect(error)}")
                Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, error})
                PoolFacade.broadcast_event("WatcherFileIndexed", %{path: path, status: "error"})
            end
          else
            Logger.debug("[Oban] Skipping binary file #{path}")
            Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, :skipped_binary)
          end
        {:error, reason} ->
          Logger.error("[Oban] Could not read file #{path}: #{inspect(reason)}")
          Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, reason})
      end
      
      # Cooperative Yielding: Micro-pause to let the OS scheduler breathe
      Process.sleep(2)
    end)
    :ok
  end
end
