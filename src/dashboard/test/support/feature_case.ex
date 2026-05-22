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

      alias Wallaby.Browser
      alias Wallaby.Query

      @moduletag :feature
      @endpoint AxonDashboardWeb.Endpoint
    end
  end
end
