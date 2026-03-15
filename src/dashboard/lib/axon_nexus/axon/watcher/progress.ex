defmodule Axon.Watcher.Progress do
  @moduledoc """
  Factual reporting of indexing progress to HydraDB (Pod C).
  """
  require Logger

  @hydra_host {127, 0, 0, 1}
  @hydra_port 6040
  @api_key "dev_key"

  def update_status(repo_slug, status_map) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    metadata = %{
      "status" => status_map[:status] || "live",
      "progress" => status_map[:progress] || 0,
      "synced" => status_map[:synced] || 0,
      "total" => status_map[:total] || 0,
      "last_update" => now,
      "last_scan_at" => status_map[:last_scan_at] || now,
      "last_file_import_at" => status_map[:last_file_import_at] || now
    }

    # 1. Reliable local reporting (File)
    write_local_status(repo_slug, metadata)

    # 2. Centralized reporting (HydraDB) - Async attempt
    Task.start(fn ->
      key = "axon:repo:#{repo_slug}"
      send_to_hydradb("put", %{"key" => key, "value" => metadata})
    end)
  end

  def get_status(repo_slug) do
    # 1. Try local status first (FAST & RELIABLE)
    home = System.user_home!()
    status_path = Path.join([home, ".axon", "repos", repo_slug, "status.json"])

    case File.read(status_path) do
      {:ok, json} ->
        case Jason.decode(json) do
          {:ok, data} -> data
          _ -> fetch_from_hydradb(repo_slug)
        end

      _ ->
        fetch_from_hydradb(repo_slug)
    end
  end

  defp fetch_from_hydradb(repo_slug) do
    key = "axon:repo:#{repo_slug}"

    case sync_send_to_hydradb("get", %{"key" => key}) do
      {:ok, %{"status" => "ok", "value" => data}} -> data
      _ -> %{"status" => "offline", "progress" => 0, "synced" => 0, "total" => 0}
    end
  end

  def get_directory_stats(_repo_slug) do
    # On récupère tous les fichiers via une requête Cypher sur Pod C
    case sync_send_to_hydradb("keys", %{"pattern" => "axon:mtime:*"}) do
      {:ok, %{"status" => "ok", "keys" => keys}} ->
        keys
        |> Enum.map(fn key ->
          parts = String.split(key, ":")
          path = List.last(parts)
          Path.relative_to(path, File.cwd!()) |> Path.split() |> List.first()
        end)
        |> Enum.reject(&is_nil/1)
        |> Enum.frequencies()

      _ ->
        %{}
    end
  end

  def purge_repo(repo_slug) do
    # On supprime les métadonnées et tous les mtimes associés au slug
    sync_send_to_hydradb("delete", %{"key" => "axon:repo:#{repo_slug}"})
    # Pour les mtimes, comme on n'a pas de delete by pattern natif simple ici, 
    # on pourrait itérer ou envoyer une commande cypher si Pod C le permet.
    # On se contente du repo status pour l'UI pour l'instant.
    Logger.warning("[Progress] Knowledge base purge requested for #{repo_slug}")
  end

  def get_file_mtime(repo_slug, file_path) do
    key = "axon:mtime:#{repo_slug}:#{file_path}"

    case sync_send_to_hydradb("get", %{"key" => key}) do
      {:ok, %{"status" => "ok", "value" => mtime}} -> mtime
      _ -> 0
    end
  end

  def save_file_mtime(repo_slug, file_path, mtime) do
    key = "axon:mtime:#{repo_slug}:#{file_path}"

    Task.start(fn ->
      send_to_hydradb("put", %{"key" => key, "value" => mtime})
    end)
  end

  defp write_local_status(repo_slug, data) do
    home = System.user_home!()
    status_path = Path.join([home, ".axon", "repos", repo_slug, "status.json"])

    case Jason.encode(data) do
      {:ok, json} ->
        File.mkdir_p!(Path.dirname(status_path))
        File.write(status_path, json)

      _ ->
        :ok
    end
  end

  defp sync_send_to_hydradb(op, args) do
    case :gen_tcp.connect(@hydra_host, @hydra_port, [:binary, packet: 4, active: false], 5000) do
      {:ok, socket} ->
        :gen_tcp.send(socket, Msgpax.pack!(%{"auth" => @api_key}))

        case :gen_tcp.recv(socket, 0, 5000) do
          {:ok, _auth_resp} ->
            payload = %{"op" => op} |> Map.merge(args)
            :gen_tcp.send(socket, Msgpax.pack!(payload))

            case :gen_tcp.recv(socket, 0, 5000) do
              {:ok, data} ->
                :gen_tcp.close(socket)
                {:ok, Msgpax.unpack!(data)}

              _ ->
                :gen_tcp.close(socket)
                :error
            end

          _ ->
            :gen_tcp.close(socket)
            :error
        end

      _ ->
        :error
    end
  end

  defp send_to_hydradb(op, args) do
    case :gen_tcp.connect(@hydra_host, @hydra_port, [:binary, packet: 4, active: false], 5000) do
      {:ok, socket} ->
        :gen_tcp.send(socket, Msgpax.pack!(%{"auth" => @api_key}))

        case :gen_tcp.recv(socket, 0, 5000) do
          {:ok, _auth_resp} ->
            payload = %{"op" => op} |> Map.merge(args)
            :gen_tcp.send(socket, Msgpax.pack!(payload))
            :gen_tcp.close(socket)
            :ok

          {:error, reason} ->
            Logger.error("[Progress] Auth failed with HydraDB: #{inspect(reason)}")
            :gen_tcp.close(socket)
        end

      {:error, reason} ->
        # Log only in debug to avoid pollution, as this is best-effort async reporting
        Logger.debug("[Progress] HydraDB offline at #{@hydra_port}: #{inspect(reason)}")
    end
  end
end
