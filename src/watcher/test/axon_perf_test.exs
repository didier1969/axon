defmodule Axon.Watcher.PerfTest do
  use ExUnit.Case
  alias Axon.Watcher.PoolFacade

  test "measure parsing latency over the pool" do
    content = Enum.map(1..100, fn i -> "def func_#{i}(): return #{i}" end) |> Enum.join("\n")
    
    # Préchauffage (Cold start uv)
    PoolFacade.ping()
    
    start_time = System.monotonic_time(:microsecond)
    %{"status" => "ok"} = PoolFacade.parse("perf.py", content)
    end_time = System.monotonic_time(:microsecond)
    
    duration = (end_time - start_time) / 1000
    IO.puts("\n--- ⚡ Performance Report (MsgPack + Pool) ---")
    IO.puts("Parsing duration for 100 functions: #{duration} ms")
    IO.puts("Average per-function latency: #{duration / 100} ms")
    
    assert duration < 500 # Le parsing pur doit être très rapide après le démarrage
  end
end
