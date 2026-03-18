defmodule LiveView.Witness.HTML do
  @moduledoc """
  HTML helpers and components for LiveView.Witness.
  """
  use Phoenix.Component
  import Phoenix.HTML, only: [raw: 1]

  @doc """
  Generates the Vanilla JS Survival Watchdog script.

  ## Options

    * `:oracle_url` - The URL of the Oracle endpoint (default: `"/liveview_witness/diagnose"`).
    * `:timeout` - The timeout in milliseconds to wait for LiveView connection (default: `5000`).

  """
  def watchdog_script(opts \\ []) do
    oracle_url = Keyword.get(opts, :oracle_url, "/liveview_witness/diagnose")
    timeout = Keyword.get(opts, :timeout, 5000)
    token = LiveView.Witness.Token.get()

    raw("""
    <!-- LiveView.Witness - Survival Watchdog -->
    <script>
      (function() {
        const ORACLE_URL = "#{oracle_url}";
        const WITNESS_TOKEN = "#{token}";
        const pingOracle = (data) => {
          console.warn("[Witness.Watchdog] Survival trigger:", data);
          fetch(ORACLE_URL, {
            method: "POST",
            headers: { 
              "Content-Type": "application/json",
              "X-Witness-Token": WITNESS_TOKEN
            },
            body: JSON.stringify({ ...data, watchdog: true, url: window.location.href })
          }).catch(() => {});
        };

        // Capture early JS errors
        window.addEventListener("error", (e) => {
          if (!window.liveSocket) {
            pingOracle({ type: "bootstrap_error", message: e.message, source: e.filename });
          }
        });

        // Connection Watchdog: if not live after #{timeout}ms, something is wrong
        setTimeout(() => {
          if (!window.liveSocket || !window.liveSocket.isConnected()) {
            const is500 = document.title.includes("Internal Server Error") ||
                          document.body.innerText.includes("Internal Server Error");
            pingOracle({
              type: is500 ? "server_error_500" : "connection_timeout",
              message: is500 ? "Server returned 500 or crash page" : "WebSocket failed to connect after #{timeout}ms"
            });
          }
        }, #{timeout});
      })();
    </script>
    """)
  end

  @doc """
  Renders a container div with `phx-hook="LiveViewWitness"`.
  """
  attr :id, :string, default: "witness-container"
  attr :class, :string, default: nil
  slot :inner_block, required: true

  def witness_container(assigns) do
    ~H"""
    <div id={@id} phx-hook="LiveViewWitness" class={@class}>
      {render_slot(@inner_block)}
    </div>
    """
  end
end
