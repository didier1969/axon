defmodule Axon.Watcher.Worker do
  @moduledoc """
  A GenServer managing exactly one Python Slave.
  Multiplied by PartitionSupervisor to create a scalable pool.
  """
  use GenServer
  require Logger

  # --- Client API ---

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts)
  end

  def parse(worker_pid, path, content, timeout \\ 30_000) do
    GenServer.call(worker_pid, {:command, %{"command" => "parse", "path" => path, "content" => content}}, timeout)
  end
  
  def parse_batch(worker_pid, files, timeout \\ 30_000) do
    GenServer.call(worker_pid, {:command, %{"command" => "parse_batch", "files" => files}}, timeout)
  end

  def ping(worker_pid, timeout \\ 5000) do
    GenServer.call(worker_pid, {:command, %{"command" => "ping"}}, timeout)
  end

  # --- Server Callbacks ---

  @impl true
  def init(_opts) do
    uv_exec = System.find_executable("uv")
    
    root = __DIR__ 
           |> Path.join("../../../../../") 
           |> Path.expand()
    worker_path = Path.join(root, "src/axon/bridge/worker.py")

    # Ouverture du port en mode binaire avec 4 octets de taille (Magie Erlang)
    port = Port.open({:spawn_executable, uv_exec}, [
      :binary,
      :exit_status,
      {:packet, 4},
      args: ["run", "python", worker_path]
    ])

    {:ok, %{port: port, pending_calls: :queue.new()}}
  end

  @impl true
  def handle_call({:command, payload}, from, state) do
    binary_payload = Msgpax.pack!(payload) |> IO.iodata_to_binary()
    Port.command(state.port, binary_payload)
    
    # On ajoute l'appelant dans la queue pour gérer les requêtes asynchrones
    new_queue = :queue.in(from, state.pending_calls)
    {:noreply, %{state | pending_calls: new_queue}}
  end

  @impl true
  def handle_info({port, {:data, data}}, %{port: port} = state) do
    # On dépile le plus ancien appelant
    {{:value, caller}, new_queue} = :queue.out(state.pending_calls)
    
    response = Msgpax.unpack!(data)
    GenServer.reply(caller, response)

    {:noreply, %{state | pending_calls: new_queue}}
  end

  @impl true
  def handle_info({port, {:exit_status, status}}, %{port: port} = state) do
    Logger.error("Python worker died with status #{status}")
    {:stop, :worker_died, state}
  end
end
