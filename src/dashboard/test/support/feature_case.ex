defmodule AxonDashboardWeb.FeatureCase do
  @moduledoc """
  ExUnit case template for Wallaby-driven browser feature tests
  (REQ-AXO-901649).

  Each test gets a fresh Wallaby session against the headless Chrome
  driven by ChromeDriver. The endpoint is already started by the
  `:axon_dashboard` app (test.exs has `server: true`).

  Tagged `:feature` so the suite can be selected via
  `mix test --only feature`.
  """

  use ExUnit.CaseTemplate

  using do
    quote do
      use Wallaby.Feature

      import Wallaby.Browser
      import Wallaby.Query, only: [css: 1, css: 2, link: 1, button: 1, text_field: 1, xpath: 1]
      import AxonDashboardWeb.FeatureCase, only: [wait_for_live: 1, wait_for_live: 2]

      alias Wallaby.Browser
      alias Wallaby.Query

      @moduletag :feature
      @endpoint AxonDashboardWeb.Endpoint
    end
  end

  @doc """
  REQ-AXO-901683 — wait until the Phoenix LiveView WebSocket finished
  connecting in the headless Chromium driven by Wallaby. Phoenix LV
  sets `class="phx-connected"` on the root `data-phx-main` element
  once the socket has handshaked.

  Tests that drive `phx-click` / `phx-keyup` events must call this
  AFTER `visit(...)`, otherwise clicks land on a disconnected DOM and
  the LiveView server never sees the event (Wallaby has no built-in
  wait for LV connection).
  """
  def wait_for_live(session, timeout_ms \\ 5_000) do
    deadline = System.monotonic_time(:millisecond) + timeout_ms
    do_wait_for_live(session, deadline)
  end

  defp do_wait_for_live(session, deadline) do
    if live_connected?(session) do
      session
    else
      if System.monotonic_time(:millisecond) >= deadline do
        # Fallback : page may be purely static (no LV) — don't block.
        session
      else
        Process.sleep(50)
        do_wait_for_live(session, deadline)
      end
    end
  end

  defp live_connected?(session) do
    ref = make_ref()
    parent = self()

    Wallaby.Browser.execute_script(
      session,
      """
      var el = document.querySelector('[data-phx-main]');
      return el && el.classList.contains('phx-connected') ? "yes" : "no";
      """,
      [],
      fn value -> send(parent, {ref, value}) end
    )

    receive do
      {^ref, "yes"} -> true
      {^ref, _} -> false
    after
      2_000 -> false
    end
  end
end
