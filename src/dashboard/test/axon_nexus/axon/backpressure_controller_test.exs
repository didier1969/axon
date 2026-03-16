defmodule Axon.BackpressureControllerTest do
  use ExUnit.Case, async: false

  alias Axon.BackpressureController

  defmodule MockResourceMonitor do
    def get_system_load do
      Agent.get(__MODULE__, & &1)
    end

    def set_load(cpu, ram, io \\ 0.0) do
      Agent.update(__MODULE__, fn _ -> %{cpu: cpu, ram: ram, io: io} end)
    end
  end

  defmodule MockOban do
    def pause_queue(queue: q), do: send(:test_pid, {:oban_pause, q})
    def resume_queue(queue: q), do: send(:test_pid, {:oban_resume, q})
    def scale_queue(queue: q, limit: l), do: send(:test_pid, {:oban_scale, q, l})
  end

  setup do
    Process.register(self(), :test_pid)
    {:ok, _pid} = Agent.start_link(fn -> %{cpu: 10.0, ram: 10.0, io: 0.0} end, name: MockResourceMonitor)

    # Ensure consistent test environment config
    Application.put_env(:axon_dashboard, Axon.BackpressureController,
      cpu_hard_limit: 70.0,
      ram_hard_limit: 70.0,
      io_hard_limit: 20.0
    )
    
    :ok
  end

  test "scales to 10 when pressure is under 50% (e.g. IO=5/20=0.25)" do
    MockResourceMonitor.set_load(30.0, 30.0, 5.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_1,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_scale, :indexing_default, 10}
    assert_receive {:oban_scale, :indexing_hot, 5}

    GenServer.stop(pid)
  end

  test "scales to 5 when pressure is between 50% and 75% (e.g. IO=12/20=0.60)" do
    MockResourceMonitor.set_load(30.0, 30.0, 12.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_2,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_scale, :indexing_default, 5}
    assert_receive {:oban_scale, :indexing_hot, 2}

    GenServer.stop(pid)
  end

  test "scales to 1 when pressure is between 75% and 100% (e.g. CPU=60/70=0.85)" do
    MockResourceMonitor.set_load(60.0, 10.0, 5.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_3,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_scale, :indexing_default, 1}
    assert_receive {:oban_scale, :indexing_hot, 1}

    GenServer.stop(pid)
  end

  test "pauses queues when IO hits 20% hard limit (pressure >= 1.0)" do
    MockResourceMonitor.set_load(10.0, 10.0, 20.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_4,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_pause, :indexing_default}
    assert_receive {:oban_pause, :indexing_hot}

    GenServer.stop(pid)
  end

  test "resumes queues when load recovers from >100% pressure to <100%" do
    # Initial state: Paused due to RAM = 75/70
    MockResourceMonitor.set_load(10.0, 75.0, 0.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_5,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_pause, :indexing_default}
    assert_receive {:oban_pause, :indexing_hot}

    # Recover load: IO=5/20=0.25 (Pressure < 0.5)
    MockResourceMonitor.set_load(10.0, 10.0, 5.0)
    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_resume, :indexing_default}
    assert_receive {:oban_resume, :indexing_hot}
    assert_receive {:oban_scale, :indexing_default, 10}

    GenServer.stop(pid)
  end

  test "get_chunk_size returns correct size based on pressure" do
    # Pressure 0.25 (< 0.50) -> 100
    MockResourceMonitor.set_load(10.0, 10.0, 5.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 100

    # Pressure 0.60 (< 0.75) -> 50
    MockResourceMonitor.set_load(10.0, 10.0, 12.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 50

    # Pressure 0.85 (< 1.00) -> 10
    MockResourceMonitor.set_load(60.0, 10.0, 5.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 10

    # Pressure 1.0 (>= 1.00) -> 5
    MockResourceMonitor.set_load(10.0, 10.0, 20.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 5
  end
end
