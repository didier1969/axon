defmodule Axon.BackpressureControllerTest do
  use ExUnit.Case, async: false

  alias Axon.BackpressureController

  defmodule MockResourceMonitor do
    def get_system_load do
      Agent.get(__MODULE__, & &1)
    end

    def set_load(cpu, ram) do
      Agent.update(__MODULE__, fn _ -> %{cpu: cpu, ram: ram} end)
    end
  end

  defmodule MockOban do
    def pause_queue(queue: q), do: send(:test_pid, {:oban_pause, q})
    def resume_queue(queue: q), do: send(:test_pid, {:oban_resume, q})
    def scale_queue(queue: q, limit: l), do: send(:test_pid, {:oban_scale, q, l})
  end

  setup do
    Process.register(self(), :test_pid)
    {:ok, _pid} = Agent.start_link(fn -> %{cpu: 10.0, ram: 10.0} end, name: MockResourceMonitor)
    :ok
  end

  test "scales to 10 when load is under 20%" do
    MockResourceMonitor.set_load(15.0, 10.0)

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

  test "scales to 5 when load is between 20% and 30%" do
    MockResourceMonitor.set_load(25.0, 29.0)

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

  test "scales to 1 when load is between 30% and 40%" do
    MockResourceMonitor.set_load(35.0, 10.0)

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

  test "pauses queues when load hits 40% hard limit" do
    MockResourceMonitor.set_load(40.0, 10.0)

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

  test "resumes queues when load recovers from >40% to <40%" do
    MockResourceMonitor.set_load(50.0, 50.0)

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

    # Recover load
    MockResourceMonitor.set_load(15.0, 15.0)
    GenServer.call(pid, :trigger_poll)

    assert_receive {:oban_resume, :indexing_default}
    assert_receive {:oban_resume, :indexing_hot}
    assert_receive {:oban_scale, :indexing_default, 10}

    GenServer.stop(pid)
  end

  test "get_chunk_size returns correct size based on load" do
    MockResourceMonitor.set_load(15.0, 10.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 100

    MockResourceMonitor.set_load(25.0, 10.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 50

    MockResourceMonitor.set_load(35.0, 10.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 10

    MockResourceMonitor.set_load(45.0, 10.0)
    assert BackpressureController.get_chunk_size(MockResourceMonitor) == 5
  end
end
