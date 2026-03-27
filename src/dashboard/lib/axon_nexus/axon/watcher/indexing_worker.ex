defmodule Axon.Watcher.IndexingWorker do
  use Oban.Worker, queue: :indexing_default, max_attempts: 3
  require Logger
  alias Axon.Watcher.PoolFacade

  @impl true
  def perform(%Oban.Job{args: %{"batch" => batch}, id: job_id, queue: queue_name}) do
    Logger.info(
      "[Oban] Processing batch of #{length(batch)} files (Job #{job_id}) on queue #{queue_name}"
    )

    lane = if queue_name == "indexing_titan", do: "titan", else: "fast"

    # Optimization: Filter out ignored extensions and titan files before batching
    {ignored, valid} = Enum.split_with(batch, fn file ->
      ext = Path.extname(file["path"]) |> String.downcase()
      ext in [".csv", ".log", ".tar.gz"]
    end)

    # Handle ignored
    Enum.each(ignored, fn file ->
      path = file["path"]
      Axon.Watcher.Telemetry.report_finish("oban:#{job_id}", path, :skipped_binary)
      Axon.Watcher.Tracking.mark_file_status!(path, "ignored_by_rule", %{error_reason: "Ignored extension"})
    end)

    # Dispatch valid batch to Rust in ONE CALL
    if valid != [] do
      # Add t1 (processing start) to each file in batch
      t1 = :os.system_time(:microsecond)
      batch_payload = Enum.map(valid, fn f -> Map.merge(f, %{"lane" => lane, "t1" => t1}) end)

      case PoolFacade.parse_batch(batch_payload) do
        %{"status" => "ok"} ->
          # Batch completed successfully. Individual file stats are updated by PoolFacade via mark_files_status_batch!
          # We still need to increment global StatsCache for each SUCCESSFUL file in the result.
          # Note: PoolFacade could return individual results if needed.
          :ok

        error ->
          Logger.error("[Oban] Batch failure: #{inspect(error)}")
          raise "Batch Processing Error"
      end
    end

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
