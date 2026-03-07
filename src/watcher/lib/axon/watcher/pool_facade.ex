defmodule Axon.Watcher.PoolFacade do
  @moduledoc """
  Helper module to route commands to the correct worker in the PartitionSupervisor pool.
  """

  # Routing is based on a key (e.g. the file path) so the same file always goes to the same worker.
  # This prevents race conditions if a file is rapidly modified.
  
  def parse(path, content) do
    worker_pid = get_worker(path)
    Axon.Watcher.Worker.parse(worker_pid, path, content)
  end

  def parse_batch(files) do
    # For a batch, we route based on the first file's path (or a random key)
    key = if length(files) > 0, do: hd(files)["path"], else: "batch"
    worker_pid = get_worker(key)
    Axon.Watcher.Worker.parse_batch(worker_pid, files)
  end

  def ping() do
    # Ping a random worker to check health
    worker_pid = get_worker(:erlang.unique_integer())
    Axon.Watcher.Worker.ping(worker_pid)
  end

  defp get_worker(routing_key) do
    # This automatically finds the correct PID in the PartitionSupervisor based on the key
    {:via, PartitionSupervisor, {Axon.Watcher.WorkerPool, routing_key}}
  end
end
