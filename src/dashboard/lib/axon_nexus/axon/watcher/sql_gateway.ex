defmodule Axon.Watcher.SqlGateway do
  @moduledoc false

  @sql_gateway "http://127.0.0.1:44129/sql"

  def query_json(query) do
    headers = [{~c"content-type", ~c"application/json"}]
    body = Jason.encode!(%{"query" => query})

    case :httpc.request(
           :post,
           {to_charlist(@sql_gateway), headers, ~c"application/json", body},
           [timeout: 5000],
           []
         ) do
      {:ok, {{_version, 200, _reason}, _headers, response_body}} ->
        {:ok, List.to_string(response_body)}

      {:ok, {{_version, code, reason}, _headers, _body}} ->
        {:error, "HTTP #{code}: #{reason}"}

      {:error, reason} ->
        {:error, reason}
    end
  end
end
