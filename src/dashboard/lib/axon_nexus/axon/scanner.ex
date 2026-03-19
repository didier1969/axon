defmodule Axon.Scanner do
  use Rustler, otp_app: :axon_dashboard, crate: "axon_scanner"

  def scan(path) do
    scan(path, get_supported_extensions())
  end

  def start_streaming(path, pid) do
    start_streaming(path, pid, get_supported_extensions())
  end

  def scan(_path, _extensions), do: :erlang.nif_error(:nif_not_loaded)
  def start_streaming(_path, _pid, _extensions), do: :erlang.nif_error(:nif_not_loaded)

  defp get_supported_extensions do
    config_path = Path.expand("../../../../../.axon/capabilities.toml", __DIR__)

    case File.read(config_path) do
      {:ok, content} ->
        case Toml.decode(content) do
          {:ok, %{"indexing" => %{"supported_extensions" => exts}}} -> exts
          _ -> default_extensions()
        end

      _ ->
        default_extensions()
    end
  end

  defp default_extensions do
    [
      "py", "ex", "exs", "rs", "go", "java", "c", "cpp", "h",
      "js", "jsx", "ts", "tsx", "sql", "md", "markdown",
      "txt", "json", "yml", "yaml", "toml", "conf", "html", "css"
    ]
  end
end
