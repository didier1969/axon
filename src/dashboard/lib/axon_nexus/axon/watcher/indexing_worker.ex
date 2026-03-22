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

      ext = Path.extname(path) |> String.downcase()
      ignored_extensions = [".csv", ".log", ".tar.gz"]

      if ext in ignored_extensions do
        Logger.debug("[Oban] Skipping ignored extension #{path}")
        Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, :skipped_binary)
        try do
          Axon.Watcher.Tracking.mark_file_status!(path, "ignored_by_rule", %{error_reason: "Ignored extension"})
        rescue
          _ -> :ok
        end
      else
        case File.stat(path) do
          {:ok, stat} ->
            if stat.size > 5_242_880 do # 5 MB
              Logger.warning("[Oban] File too large (#{stat.size} bytes): #{path}")
              Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, :file_too_large})
              try do
                Axon.Watcher.Tracking.mark_file_status!(path, "ignored_by_rule", %{error_reason: "File too large"})
              rescue
                _ -> :ok
              end
            else
              case PoolFacade.parse(path) do
                %{"status" => "ok"} ->
                  Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, :ok)
                  
                  try do
                    Axon.Watcher.Tracking.mark_file_status!(path, "indexed")
                  rescue
                    _ -> :ok
                  end

                  Phoenix.PubSub.broadcast(
                    AxonDashboard.PubSub,
                    "bridge_events",
                    {:file_indexed, path, :ok}
                  )

                error ->
                  Logger.error("[Oban] Failed to parse #{path}: #{inspect(error)}")
                  Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, error})
                  
                  try do
                    Axon.Watcher.Tracking.mark_file_status!(path, "failed", %{error_reason: inspect(error)})
                  rescue
                    _ -> :ok
                  end

                  Phoenix.PubSub.broadcast(
                    AxonDashboard.PubSub,
                    "bridge_events",
                    {:file_indexed, path, :error}
                  )
              end
            end

          {:error, reason} ->
            Logger.error("[Oban] Could not stat file #{path}: #{inspect(reason)}")
            Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, reason})
        end
      end

      # Cooperative Yielding: Micro-pause to let the OS scheduler breathe
      # Reduced to 2ms to keep ingestion smooth but avoid artificially delaying huge batches.
      Process.sleep(2)
    end)

    :ok
  end
end
