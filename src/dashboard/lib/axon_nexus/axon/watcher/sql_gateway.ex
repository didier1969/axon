defmodule Axon.Watcher.SqlGateway do
  @moduledoc false

  @default_sql_url "http://127.0.0.1:44129/sql"
  @default_mcp_url "http://127.0.0.1:44129/mcp"

  def query_json(query) do
    headers = [{~c"content-type", ~c"application/json"}]
    body = Jason.encode!(%{"query" => query})
    primary_url = sql_gateway_url()

    case request_json(primary_url, body, 5000) do
      {:ok, _response} = ok ->
        ok

      {:error, reason} ->
        if primary_url != @default_sql_url do
          case request_json(@default_sql_url, body, 5000) do
            {:ok, _response} = ok -> ok
            {:error, _fallback_reason} -> {:error, reason}
          end
        else
          {:error, reason}
        end
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
        if primary_url != @default_mcp_url do
          case request_raw(@default_mcp_url, headers, body, 2000) do
            {:ok, {{_version, 200, _reason}, _headers, _response_body}} ->
              {:ok, System.monotonic_time(:millisecond) - started_at}

            {:ok, {{_version, code, fallback_reason}, _headers, _body}} ->
              {:error, "HTTP #{code}: #{fallback_reason}", System.monotonic_time(:millisecond) - started_at}

            {:error, _fallback_error} ->
              {:error, reason, System.monotonic_time(:millisecond) - started_at}
          end
        else
          {:error, reason, System.monotonic_time(:millisecond) - started_at}
        end
    end
  end

  defp sql_gateway_url do
    [
      Application.get_env(:axon_dashboard, __MODULE__, []) |> Keyword.get(:url),
      System.get_env("AXON_SQL_URL"),
      System.get_env("SQL_URL"),
      @default_sql_url
    ]
    |> Enum.map(&sanitize_url_candidate/1)
    |> Enum.reject(fn value ->
      is_binary(value) && String.trim(value) == ""
    end)
    |> Enum.find(fn
      value when is_binary(value) and byte_size(value) > 0 -> true
      _ -> false
    end)
    |> normalize_sql_url()
  end

  def source_info do
    configured_url = sql_gateway_url()
    %{
      configured_url: configured_url,
      provider:
        if(configured_url == @default_sql_url, do: "default", else: "environment"),
      endpoint: normalize_sql_host(configured_url)
    }
  end

  defp sanitize_url_candidate(value) when is_binary(value), do: String.trim(value)
  defp sanitize_url_candidate(_), do: ""

  defp normalize_sql_host(url) when is_binary(url) do
    uri = URI.parse(url)

    if is_nil(uri.host) or is_nil(uri.port) do
      url
    else
      "#{uri.host}:#{uri.port}"
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

  defp normalize_sql_url(_url) do
    @default_sql_url
  end

  defp mcp_gateway_url do
    case URI.parse(sql_gateway_url()) do
      %URI{scheme: scheme, host: host, port: port} when is_binary(scheme) and is_binary(host) and is_integer(port) ->
        %URI{scheme: scheme, host: host, port: port, path: "/mcp"}
        |> URI.to_string()

      _ ->
        @default_mcp_url
    end
  end

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
