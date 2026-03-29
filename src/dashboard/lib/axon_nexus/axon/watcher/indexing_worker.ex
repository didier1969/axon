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

    # Optimization: Filter out ignored extensions
    {ignored, valid} = Enum.split_with(batch, fn file ->
      ext = Path.extname(file["path"]) |> String.downcase()
      ext in [".csv", ".log", ".tar.gz", ".zip", ".png", ".jpg", ".jpeg", ".pdf"]
    end)

    # Handle ignored
    Enum.each(ignored, fn file ->
      path = file["path"]
      # Record as skipped in stats cache if needed, but DuckDB is master
      :ok
    end)

    # Dispatch valid batch to Rust in ONE CALL
    if valid != [] do
      t1 = :os.system_time(:microsecond)
      batch_payload = Enum.map(valid, fn f -> Map.merge(f, %{"lane" => lane, "t1" => t1}) end)

      case PoolFacade.parse_batch(batch_payload) do
        :ok -> {:ok, :success}
        {:ok, _} -> {:ok, :success}
        {:error, reason} -> 
          Logger.error("[Oban] Batch failure for Job #{job_id}: #{inspect(reason)}")
          {:error, reason}
        other -> 
          Logger.warning("[Oban] Batch Job #{job_id} unexpected return: #{inspect(other)}")
          {:ok, other}
      end
    else
      {:ok, :empty_batch}
    end
  end
end
