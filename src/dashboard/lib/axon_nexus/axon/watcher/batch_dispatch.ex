defmodule Axon.Watcher.BatchDispatch do
  @moduledoc false

  require Logger

  def dispatch(paths, queue) do
    files_payload =
      Enum.map(paths, fn path ->
        %{
          "path" => path,
          "trace_id" => Ecto.UUID.generate(),
          "t0" => :os.system_time(:microsecond)
        }
      end)

    if length(files_payload) > 0 do
      try do
        job_args = %{"batch" => files_payload}

        Axon.Watcher.IndexingWorker.new(job_args, queue: queue)
        |> Oban.insert!()

        :telemetry.execute([:axon, :watcher, :batch_enqueued], %{count: length(files_payload)}, %{
          queue: queue
        })

        Logger.info("[Pod A] Enqueued batch of #{length(files_payload)} files to #{queue}.")
      rescue
        e ->
          :telemetry.execute([:axon, :watcher, :batch_failed], %{count: length(files_payload)}, %{
            queue: queue,
            error: inspect(e)
          })

          Logger.error("[Pod A] FAILED to enqueue batch: #{inspect(e)}")
      end
    end
  end
end
