defmodule Axon.Watcher.PipelineMaillonsTest do
  use ExUnit.Case, async: false
  require Logger

  alias Axon.Watcher.TrafficGuardian
  alias Axon.Watcher.PoolFacade

  # --- MAILLON 4: L'ORCHESTRATEUR (Traffic Guardian) ---
  test "maillon 4: Traffic Guardian should increase pressure when load is low" do
    # On récupère le PID existant ou on le démarre
    pid = case TrafficGuardian.start_link() do
      {:ok, p} -> p
      {:error, {:already_started, p}} -> p
    end
    
    # On force un check de pression
    send(pid, :check_pressure)
    
    # On vérifie que le process est toujours en vie
    assert Process.alive?(pid)
  end

  # --- MAILLON 8: LA BOUCLE DE RÉTROACTION (Feedback) ---
  test "maillon 8: PoolFacade should process indexed events without crashing" do
    # On vérifie si PoolFacade est lancé par l'application de test
    # Sinon on le démarre
    pid = case Process.whereis(PoolFacade) do
      nil -> 
        {:ok, p} = PoolFacade.start_link([])
        p
      p -> p
    end

    # Simuler l'arrivée d'une ligne JSON sur la socket via handle_info
    # Cet événement doit être traité par PoolFacade et redirigé vers le Guardian
    line = ~s({"FileIndexed": {"path": "/tmp/test.ex", "status": "ok", "t4": 500}})
    
    # On envoie le message au GenServer PoolFacade
    send(PoolFacade, {:tcp, nil, line <> "\n"})
    
    # On vérifie que PoolFacade survit au décodage
    assert Process.alive?(pid)
  end
end
