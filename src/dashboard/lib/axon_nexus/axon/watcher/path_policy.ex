defmodule Axon.Watcher.PathPolicy do
  @moduledoc false

  def should_process?(path) do
    not (String.contains?(path, "/.git/") or
           String.contains?(path, "/.axon/") or
           String.contains?(path, "/_build/") or
           String.contains?(path, "/deps/") or
           String.contains?(path, "/.devenv/") or
           String.contains?(path, "/node_modules/") or
           String.contains?(path, "/target/"))
  end

  def calculate_priority(path) do
    ext = Path.extname(path) |> String.downcase()

    cond do
      ext in [".ex", ".exs", ".rs", ".py", ".go"] -> 100
      ext in [".js", ".ts", ".c", ".cpp", ".h"] -> 80
      ext in [".md", ".txt", ".json", ".yml", ".yaml", ".toml", ".conf"] -> 50
      true -> 10
    end
  end

  def get_top_dir(path, watch_dir) do
    abs_path = Path.expand(path)
    abs_watch_dir = Path.expand(watch_dir)

    if String.starts_with?(abs_path, abs_watch_dir) do
      relative_path =
        abs_path
        |> String.replace_prefix(abs_watch_dir, "")
        |> String.trim_leading("/")

      case Path.split(relative_path) do
        [dir | _] when dir != "." and dir != "" -> dir
        _ -> "root"
      end
    else
      "external"
    end
  end
end
