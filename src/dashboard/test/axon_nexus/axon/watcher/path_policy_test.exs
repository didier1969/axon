defmodule Axon.Watcher.PathPolicyTest do
  use ExUnit.Case, async: true

  alias Axon.Watcher.PathPolicy

  test "should_process?/1 filters noisy infrastructure paths" do
    refute PathPolicy.should_process?("/tmp/repo/.git/config")
    refute PathPolicy.should_process?("/tmp/repo/deps/lib/foo.ex")
    refute PathPolicy.should_process?("/tmp/repo/target/debug/app")
    assert PathPolicy.should_process?("/tmp/repo/src/app.ex")
  end

  test "calculate_priority/1 assigns higher priority to source files" do
    assert PathPolicy.calculate_priority("lib/app.ex") == 100
    assert PathPolicy.calculate_priority("src/main.rs") == 100
    assert PathPolicy.calculate_priority("assets/app.ts") == 80
    assert PathPolicy.calculate_priority("docs/readme.md") == 50
    assert PathPolicy.calculate_priority("tmp/blob.bin") == 10
  end

  test "get_top_dir/2 resolves project root relative to watch dir" do
    watch_dir = "/tmp/workspace"

    assert PathPolicy.get_top_dir("/tmp/workspace/projects/alpha/lib/app.ex", watch_dir) == "projects"
    assert PathPolicy.get_top_dir("/tmp/other/place/app.ex", watch_dir) == "external"
  end
end
