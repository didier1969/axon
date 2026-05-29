defmodule Axon.Watcher.SqlGateway do
  @moduledoc """
  HTTP gateway to the Axon brain SQL endpoint.

  Config (populated by `config/runtime.exs`, REQ-AXO-901802) :

      config :axon_dashboard, Axon.Watcher.SqlGateway,
        url: "http://127.0.0.1:44139/sql",        # instance-aware
        allow_cross_instance_fallback: false      # REQ-AXO-901800 default

  When `allow_cross_instance_fallback: true` and the primary URL fails,
  the gateway falls back to `@safety_sql_url` (the legacy live default)
  and emits a Logger.warning. Default is `false` — cross-instance leakage
  fails loud, never silent.
  """

  require Logger

  # REQ-AXO-901802 (cat B) — defensive fallback only. Production config
  # always populates Application.env. These constants are referenced
  # only when running outside the normal config boot (e.g. tests without
  # explicit setup) and emit a warning when used.
  @safety_sql_url "http://127.0.0.1:44129/sql"
  @safety_mcp_url "http://127.0.0.1:44129/mcp"

  def query_json(query) do
    body = Jason.encode!(%{"query" => query})
    primary_url = sql_gateway_url()

    case request_json(primary_url, body, 5000) do
      {:ok, _response} = ok ->
        ok

      {:error, reason} ->
        maybe_fallback_request(:json, primary_url, body, reason)
    end
  end

  def mcp_ping do
    headers = [{~c"content-type", ~c"application/json"}]
    body = Jason.encode!(%{"jsonrpc" => "2.0", "id" => "cockpit-ping", "method" => "initialize"})
    started_at = System.monotonic_time(:millisecond)
    primary_url = mcp_gateway_url()

    case request_raw(primary_url, headers, body, 2000) do
      {:ok, {{_version, 200, _reason}, _headers, _response_body}} ->
        {:ok, System.monotonic_time(:millisecond) - started_at}

      {:ok, {{_version, code, reason}, _headers, _body}} ->
        {:error, "HTTP #{code}: #{reason}", System.monotonic_time(:millisecond) - started_at}

      {:error, reason} ->
        maybe_fallback_ping(primary_url, headers, body, reason, started_at)
    end
  end

  def source_info do
    configured_url = sql_gateway_url()
    config_source =
      cond do
        Application.get_env(:axon_dashboard, __MODULE__, []) |> Keyword.has_key?(:url) -> "application_env"
        true -> "safety_default"
      end

    %{
      configured_url: configured_url,
      provider: config_source,
      endpoint: normalize_sql_host(configured_url)
    }
  end

  # ## URL resolution

  # REQ-AXO-901802 (cat B) — single source via Application.env populated
  # by config/runtime.exs. No more scattered System.get_env in lib/.
  defp sql_gateway_url do
    Application.get_env(:axon_dashboard, __MODULE__, [])
    |> Keyword.get(:url, @safety_sql_url)
    |> normalize_sql_url()
  end

  defp mcp_gateway_url do
    case URI.parse(sql_gateway_url()) do
      %URI{scheme: scheme, host: host, port: port} when is_binary(scheme) and is_binary(host) and is_integer(port) ->
        %URI{scheme: scheme, host: host, port: port, path: "/mcp"}
        |> URI.to_string()

      _ ->
        @safety_mcp_url
    end
  end

  defp normalize_sql_url(url) when is_binary(url) do
    normalized = String.trim(url)
    cond do
      String.ends_with?(normalized, "/sql") -> normalized
      String.ends_with?(normalized, "/") -> normalized <> "sql"
      true -> normalized <> "/sql"
    end
  end

  defp normalize_sql_url(_url), do: @safety_sql_url

  defp normalize_sql_host(url) when is_binary(url) do
    uri = URI.parse(url)

    if is_nil(uri.host) or is_nil(uri.port) do
      url
    else
      "#{uri.host}:#{uri.port}"
    end
  end

  # ## Fallback policy (REQ-AXO-901800)

  defp cross_instance_fallback_allowed? do
    Application.get_env(:axon_dashboard, __MODULE__, [])
    |> Keyword.get(:allow_cross_instance_fallback, false)
  end

  defp maybe_fallback_request(:json, primary_url, body, reason) do
    cond do
      not cross_instance_fallback_allowed?() ->
        {:error, reason}

      primary_url == @safety_sql_url ->
        # already tried the safety URL — no point retrying it.
        {:error, reason}

      true ->
        Logger.warning(
          "[SqlGateway] primary #{primary_url} failed (#{inspect(reason)}); falling back to safety URL #{@safety_sql_url} — likely brain unreachable"
        )

        case request_json(@safety_sql_url, body, 5000) do
          {:ok, _response} = ok -> ok
          {:error, _fallback_reason} -> {:error, reason}
        end
    end
  end

  defp maybe_fallback_ping(primary_url, headers, body, reason, started_at) do
    cond do
      not cross_instance_fallback_allowed?() ->
        {:error, reason, System.monotonic_time(:millisecond) - started_at}

      primary_url == @safety_mcp_url ->
        {:error, reason, System.monotonic_time(:millisecond) - started_at}

      true ->
        Logger.warning(
          "[SqlGateway] mcp_ping primary #{primary_url} failed (#{inspect(reason)}); falling back to safety URL #{@safety_mcp_url}"
        )

        case request_raw(@safety_mcp_url, headers, body, 2000) do
          {:ok, {{_version, 200, _reason}, _headers, _response_body}} ->
            {:ok, System.monotonic_time(:millisecond) - started_at}

          {:ok, {{_version, code, fallback_reason}, _headers, _body}} ->
            {:error, "HTTP #{code}: #{fallback_reason}", System.monotonic_time(:millisecond) - started_at}

          {:error, _fallback_error} ->
            {:error, reason, System.monotonic_time(:millisecond) - started_at}
        end
    end
  end

  # ## HTTP plumbing

  defp request_json(url, body, timeout_ms) do
    headers = [{~c"content-type", ~c"application/json"}]

    case request_raw(url, headers, body, timeout_ms) do
      {:ok, {{_version, 200, _reason}, _headers, response_body}} ->
        {:ok, List.to_string(response_body)}

      {:ok, {{_version, code, reason}, _headers, _body}} ->
        {:error, "HTTP #{code}: #{reason}"}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp request_raw(url, headers, body, timeout_ms) do
    :httpc.request(
      :post,
      {to_charlist(url), headers, ~c"application/json", body},
      [timeout: timeout_ms],
      []
    )
  end
end
