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
    if Process.whereis(:test_pid) do
      Process.unregister(:test_pid)
    end

    Process.register(self(), :test_pid)

    case Agent.start_link(fn -> %{cpu: 10.0, ram: 10.0, io: 0.0} end, name: MockResourceMonitor) do
      {:ok, _pid} ->
        :ok

      {:error, {:already_started, pid}} ->
        Agent.stop(pid)
        {:ok, _pid} = Agent.start_link(fn -> %{cpu: 10.0, ram: 10.0, io: 0.0} end, name: MockResourceMonitor)
        :ok
    end

    # Ensure consistent test environment config
    Application.put_env(:axon_dashboard, Axon.BackpressureController,
      cpu_hard_limit: 70.0,
      ram_hard_limit: 70.0,
      io_hard_limit: 20.0
    )

    on_exit(fn ->
      if pid = Process.whereis(MockResourceMonitor) do
        if Process.alive?(pid) do
          Agent.stop(pid)
        end
      end

      if Process.whereis(:test_pid) do
        Process.unregister(:test_pid)
      end

      Application.delete_env(:axon_dashboard, Axon.BackpressureController)
    end)

    :ok
  end

  test "computes low-pressure guidance without scaling Oban queues" do
    MockResourceMonitor.set_load(30.0, 30.0, 5.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_1,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    refute_receive {:oban_scale, _, _}, 50
    refute_receive {:oban_pause, _}, 50
    refute_receive {:oban_resume, _}, 50

    GenServer.stop(pid)
  end

  test "computes medium-pressure guidance without scaling Oban queues" do
    MockResourceMonitor.set_load(30.0, 30.0, 12.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_2,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    refute_receive {:oban_scale, _, _}, 50
    refute_receive {:oban_pause, _}, 50
    refute_receive {:oban_resume, _}, 50

    GenServer.stop(pid)
  end

  test "computes high-pressure guidance without scaling Oban queues" do
    MockResourceMonitor.set_load(60.0, 10.0, 5.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_3,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    refute_receive {:oban_scale, _, _}, 50
    refute_receive {:oban_pause, _}, 50
    refute_receive {:oban_resume, _}, 50

    GenServer.stop(pid)
  end

  test "publishes constrained state without pausing Oban queues" do
    MockResourceMonitor.set_load(10.0, 10.0, 20.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_4,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)

    refute_receive {:oban_pause, _}, 50
    refute_receive {:oban_scale, _, _}, 50
    refute_receive {:oban_resume, _}, 50

    GenServer.stop(pid)
  end

  test "recovery does not resume Oban queues because Elixir is display-only" do
    MockResourceMonitor.set_load(10.0, 75.0, 0.0)

    {:ok, pid} =
      BackpressureController.start_link(
        name: :test_controller_5,
        poll_interval: 0,
        monitor_mod: MockResourceMonitor,
        oban_mod: MockOban
      )

    GenServer.call(pid, :trigger_poll)
    refute_receive {:oban_pause, _}, 50
    refute_receive {:oban_scale, _, _}, 50
    refute_receive {:oban_resume, _}, 50

    MockResourceMonitor.set_load(30.0, 30.0, 5.0)

    GenServer.call(pid, :trigger_poll)

    refute_receive {:oban_pause, _}, 50
    refute_receive {:oban_scale, _, _}, 50
    refute_receive {:oban_resume, _}, 50

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
