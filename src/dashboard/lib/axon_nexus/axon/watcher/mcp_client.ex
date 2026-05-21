defmodule Axon.Watcher.McpClient do
  @moduledoc """
  MCP (Model Context Protocol) client for the streamable-HTTP transport
  used by axon-brain.

  Uses Erlang's built-in `:httpc` (no extra deps) and handles the three
  quirks of the current axon-brain MCP HTTP endpoint:

    1. Requires `Accept: application/json, text/event-stream`.
    2. Requires an `initialize` JSON-RPC handshake on a fresh session
       and returns the `mcp-session-id` header.
    3. Responses may be plain JSON OR a single SSE `data: <json>` event,
       depending on whether the tool result includes a `content` array.

  Lifetime model: one session per call. The handshake is cheap (~10 ms)
  and we never hold the session, which avoids ghost-session bugs when
  the brain restarts.

  All entry points are blocking; callers should run them under a Task
  if they must not block the LiveView process.
  """

  require Logger

  @default_endpoint "http://127.0.0.1:44129/mcp"
  @default_timeout_ms 8_000
  @protocol_version "2024-11-05"

  @type call_result :: {:ok, term()} | {:error, term()}

  @doc """
  List all tools exposed by the MCP server.

  Returns a list of `%{"name", "description", "inputSchema"}`.
  """
  @spec list_tools(keyword) :: call_result()
  def list_tools(opts \\ []) do
    case rpc("tools/list", %{}, opts) do
      {:ok, %{"tools" => tools}} when is_list(tools) -> {:ok, tools}
      {:ok, other} -> {:error, {:bad_shape, other}}
      err -> err
    end
  end

  @doc """
  Call a tool by name. Returns the raw `result` object on success
  (with `_unwrapped` added when the tool returned a JSON-in-text payload).
  """
  @spec call_tool(String.t(), map(), keyword) :: call_result()
  def call_tool(name, arguments \\ %{}, opts \\ []) do
    rpc("tools/call", %{name: name, arguments: arguments}, opts)
  end

  ## Internals

  defp rpc(method, params, opts) do
    endpoint = Keyword.get(opts, :endpoint, endpoint())
    timeout_ms = Keyword.get(opts, :timeout_ms, @default_timeout_ms)

    with {:ok, session_id} <- initialize(endpoint, timeout_ms) do
      do_rpc(endpoint, session_id, method, params, timeout_ms)
    end
  end

  defp initialize(endpoint, timeout_ms) do
    body =
      Jason.encode!(%{
        "jsonrpc" => "2.0",
        "id" => 0,
        "method" => "initialize",
        "params" => %{
          "protocolVersion" => @protocol_version,
          "capabilities" => %{},
          "clientInfo" => %{"name" => "axon-nexus-dashboard", "version" => "2.0"}
        }
      })

    headers = [
      {~c"accept", ~c"application/json, text/event-stream"}
    ]

    request = {to_charlist(endpoint), headers, ~c"application/json", body}

    case :httpc.request(:post, request, [timeout: timeout_ms], []) do
      {:ok, {{_v, 200, _r}, resp_headers, _resp_body}} ->
        {:ok, find_session_id(resp_headers)}

      {:ok, {{_v, status, reason}, _headers, _body}} ->
        {:error, {:http, status, to_string(reason)}}

      {:error, reason} ->
        {:error, {:transport, reason}}
    end
  end

  defp do_rpc(endpoint, session_id, method, params, timeout_ms) do
    body =
      Jason.encode!(%{
        "jsonrpc" => "2.0",
        "id" => :rand.uniform(1_000_000),
        "method" => method,
        "params" => params
      })

    base_headers = [
      {~c"accept", ~c"application/json, text/event-stream"}
    ]

    headers =
      case session_id do
        nil -> base_headers
        id -> [{~c"mcp-session-id", to_charlist(id)} | base_headers]
      end

    request = {to_charlist(endpoint), headers, ~c"application/json", body}

    case :httpc.request(:post, request, [timeout: timeout_ms], []) do
      {:ok, {{_v, 200, _r}, resp_headers, resp_body}} ->
        ct = content_type(resp_headers)
        parse_response(List.to_string(resp_body), ct)

      {:ok, {{_v, status, _reason}, _headers, resp_body}} ->
        {:error, {:http, status, List.to_string(resp_body) |> String.slice(0, 300)}}

      {:error, reason} ->
        {:error, {:transport, reason}}
    end
  end

  defp parse_response(body, content_type) when is_binary(body) do
    cond do
      content_type && String.contains?(String.downcase(content_type), "text/event-stream") ->
        body
        |> String.split("\n", trim: false)
        |> Enum.reduce(nil, fn line, acc ->
          case String.split(line, ":", parts: 2) do
            ["data", rest] ->
              data = String.trim_leading(rest)
              if data in ["", "[DONE]"], do: acc, else: data

            _ ->
              acc
          end
        end)
        |> case do
          nil -> {:error, :empty_sse}
          json -> decode_rpc(json)
        end

      true ->
        decode_rpc(body)
    end
  end

  defp decode_rpc(json) do
    case Jason.decode(json) do
      {:ok, %{"result" => result}} ->
        {:ok, unwrap_tool_call(result)}

      {:ok, %{"error" => err}} ->
        {:error, {:rpc, err}}

      {:ok, other} ->
        {:error, {:bad_shape, other}}

      {:error, reason} ->
        {:error, {:decode, reason}}
    end
  end

  # Tools/call returns content=[{type:"text",text:"<markdown>"}, ...]
  # and a separate `structuredContent` field with the actual data.
  # Surface both for the UI: keep raw `result` and add `_structured`
  # as a convenience alias.
  defp unwrap_tool_call(%{"structuredContent" => sc} = result) when is_map(sc) do
    Map.put(result, "_structured", sc)
  end

  defp unwrap_tool_call(%{"content" => [%{"type" => "text", "text" => text} | _]} = result) do
    case Jason.decode(text) do
      {:ok, decoded} when is_map(decoded) -> Map.put(result, "_unwrapped", decoded)
      _ -> result
    end
  end

  defp unwrap_tool_call(result), do: result

  defp find_session_id(headers) when is_list(headers) do
    Enum.find_value(headers, fn
      {key, value} ->
        if to_string(key) |> String.downcase() == "mcp-session-id",
          do: to_string(value),
          else: nil
    end)
  end

  defp content_type(headers) when is_list(headers) do
    Enum.find_value(headers, fn
      {key, value} ->
        if to_string(key) |> String.downcase() == "content-type",
          do: to_string(value),
          else: nil
    end)
  end

  defp endpoint do
    System.get_env("AXON_MCP_ENDPOINT") || @default_endpoint
  end
end
