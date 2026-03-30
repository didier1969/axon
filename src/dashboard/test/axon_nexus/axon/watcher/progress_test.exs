defmodule Axon.Watcher.ProgressTest do
  use ExUnit.Case, async: false

  alias Axon.Watcher.Progress

  setup do
    repo_slug = "progress-test-#{System.unique_integer([:positive])}"

    on_exit(fn ->
      Progress.purge_repo(repo_slug)
    end)

    {:ok, repo_slug: repo_slug}
  end

  test "transient operator status survives without SQL gateway", %{repo_slug: repo_slug} do
    Progress.update_status(repo_slug, %{status: "indexing", progress: 42})

    status = Progress.get_status(repo_slug)

    assert status["status"] == "indexing"
    assert status["progress"] == 42
  end

  test "live status can be restored after completion", %{repo_slug: repo_slug} do
    Progress.update_status(repo_slug, %{status: "indexing", progress: 0})
    Progress.update_status(repo_slug, %{status: "live", progress: 100})

    status = Progress.get_status(repo_slug)

    assert status["status"] == "live"
    assert status["progress"] == 100
  end

  test "purge_repo removes transient overlay", %{repo_slug: repo_slug} do
    Progress.update_status(repo_slug, %{status: "indexing", progress: 10})
    Progress.purge_repo(repo_slug)

    status = Progress.get_status(repo_slug)

    assert status["status"] in ["connecting", "live"]
    refute status["progress"] == 10
  end
end
