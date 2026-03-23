# scripts/extract_elixir_deps.exs
project_dir = Enum.at(System.argv(), 0, ".")
mix_file = Path.join(project_dir, "mix.exs")

defmodule MinimalJSON do
  def encode_string(str) when is_binary(str) do
    escaped = 
      str
      |> String.replace("\\", "\\\\")
      |> String.replace("\"", "\\\"")
      |> String.replace("\n", "\\n")
      |> String.replace("\r", "\\r")
      |> String.replace("\t", "\\t")
    "\"#{escaped}\""
  end
  def encode_string(other), do: "\"#{to_string(other)}\""

  def encode_edge(%{to: to, path: path}) do
    "{\"to\": #{encode_string(to)}, \"path\": #{encode_string(path)}}"
  end

  def encode_payload(node, edges) do
    edges_json = edges |> Enum.map(&encode_edge/1) |> Enum.join(", ")
    "{\"node\": #{encode_string(node)}, \"edges\": [#{edges_json}]}"
  end
end

unless File.exists?(mix_file) do
  IO.puts(MinimalJSON.encode_payload("none", []))
  System.halt(0)
end

try do
  Mix.start()
  Code.require_file(mix_file)
  
  config = Mix.Project.config()
  app_name = config[:app] || "unknown"
  deps = config[:deps] || []
  
  edges = Enum.reduce(deps, [], fn dep, acc ->
    opts = case dep do
      {_target, opts} when is_list(opts) -> opts
      {_target, _vsn, opts} when is_list(opts) -> opts
      _ -> nil
    end

    if opts do
      target = elem(dep, 0)
      cond do
        path = opts[:path] -> 
          [%{to: to_string(target), path: Path.expand(path, project_dir)} | acc]
        opts[:in_umbrella] ->
          sibling_path = Path.expand("../#{target}", project_dir)
          [%{to: to_string(target), path: sibling_path} | acc]
        true -> acc
      end
    else
      acc
    end
  end)

  IO.puts(MinimalJSON.encode_payload(to_string(app_name), edges))
rescue
  _e -> 
    IO.puts(MinimalJSON.encode_payload("error", []))
    System.halt(1)
end
