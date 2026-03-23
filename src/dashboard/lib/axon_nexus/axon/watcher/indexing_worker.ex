defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing_default, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}, id: job_id, queue: queue_name}) do
    Logger.info("[Oban] Processing batch of #{length(batch)} files (Job #{job_id}) on queue #{queue_name}")
    
    lane = if queue_name == "indexing_titan", do: "titan", else: "fast"

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
            ram_before = get_sys_ram_mb()
            start_time = :os.system_time(:millisecond)
            
            case PoolFacade.parse(path, lane) do
                %{"status" => "ok"} ->
                  end_time = :os.system_time(:millisecond)
                  ram_after = get_sys_ram_mb()
                  duration_ms = end_time - start_time
                  
                  Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, :ok)
                  
                  try do
                    Axon.Watcher.Tracking.mark_file_status!(path, "indexed", %{
                      file_size: stat.size,
                      ingestion_duration_ms: duration_ms,
                      ram_before_mb: ram_before,
                      ram_after_mb: ram_after
                    })
                  rescue
                    _ -> :ok
                  end

                  Phoenix.PubSub.broadcast(
                    AxonDashboard.PubSub,
                    "bridge_events",
                    {:file_indexed, path, :ok}
                  )

                error ->
                  end_time = :os.system_time(:millisecond)
                  ram_after = get_sys_ram_mb()
                  duration_ms = end_time - start_time
                  
                  Logger.error("[Oban] Failed to parse #{path}: #{inspect(error)}")
                  Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, {:error, error})
                  
                  try do
                    Axon.Watcher.Tracking.mark_file_status!(path, "failed", %{
                      error_reason: inspect(error),
                      file_size: stat.size,
                      ingestion_duration_ms: duration_ms,
                      ram_before_mb: ram_before,
                      ram_after_mb: ram_after
                    })
                  rescue
                    _ -> :ok
                  end

                  Phoenix.PubSub.broadcast(
                    AxonDashboard.PubSub,
                    "bridge_events",
                    {:file_indexed, path, :error}
                  )
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

  defp get_sys_ram_mb() do
    try do
      {output, 0} = System.cmd("free", ["-m"])
      [_, mem_line | _] = String.split(output, "\n")
      [_, _total, used | _] = String.split(mem_line, " ", trim: true)
      String.to_integer(used)
    rescue
      _ -> 0
    end
  end
end
