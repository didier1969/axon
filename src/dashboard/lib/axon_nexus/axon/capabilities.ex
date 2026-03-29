defmodule Axon.Capabilities do
  @moduledoc """
  Définit les capacités d'ingestion strictes d'Axon V2.
  Permet d'implémenter la Whitelist au niveau du Watcher Elixir pour ignorer
  tous les fichiers qui ne seront de toute façon pas parsés par Tree-sitter.
  """

  @default_extensions [
    "py",
    "ex",
    "exs",
    "rs",
    "go",
    "java",
    "c",
    "cpp",
    "h",
    "js",
    "jsx",
    "ts",
    "tsx",
    "sql",
    "md",
    "markdown",
    "txt",
    "json",
    "yml",
    "yaml",
    "toml",
    "conf",
    "html",
    "css"
  ]

  @doc """
  Retourne la liste des extensions supportées (lue depuis le fichier toml ou par défaut)
  """
  def get_supported_extensions do
    config_path = Path.expand("../../../../../../.axon/capabilities.toml", __DIR__)

    case File.read(config_path) do
      {:ok, content} ->
        case Toml.decode(content) do
          {:ok, %{"indexing" => %{"supported_extensions" => exts}}} -> exts
          _ -> @default_extensions
        end

      _ ->
        @default_extensions
    end
  end

  @doc """
  Vérifie si un chemin de fichier (path) se termine par une extension supportée.
  """
  def is_supported_file?(path) do
    extension =
      path
      |> Path.extname()
      |> String.replace_leading(".", "")
      |> String.downcase()

    extension in get_supported_extensions()
  end
end
