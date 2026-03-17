defmodule LiveView.Witness.Test do
  @moduledoc """
  Macros for clean TDD flows with LiveView.Witness.

  These macros provide a unified API to assert or refute UI rendering
  both from within a LiveView (using the socket) or from a test (using the View).
  """

  import Phoenix.LiveViewTest
  import ExUnit.Assertions

  @doc """
  Asserts that a UI element is rendered according to a contract.

  When called with a `Phoenix.LiveView.Socket`, it triggers an expectation
  and synchronously waits for the client certificate.

  When called with a `Phoenix.LiveViewTest.View`, it verifies the element presence
  using `has_element?/2` and simulates a successful client certificate report.

  ## Examples

      # In a LiveView (synchronous verification)
      assert_witness_rendered(socket, ".project-grid", min_items: 1)

      # In a test
      assert_witness_rendered(view, ".project-grid")
  """
  defmacro assert_witness_rendered(view, selector, expectations \\ []) do
    quote bind_quoted: [view: view, selector: selector, expectations: expectations] do
      case view do
        %Phoenix.LiveView.Socket{} = socket ->
          {:ok, id, socket} = LiveView.Witness.expect_ui(socket, selector, expectations)
          LiveView.Witness.verify_ui!(id)
          socket

        %Phoenix.LiveViewTest.View{} = view ->
          # L1 reality check: verify element presence in the HTML
          if has_element?(view, selector) do
            # L2/L3 simulation: start Witness protocol and report success
            {:ok, id, _} = LiveView.Witness.expect_ui(view, selector, expectations)
            LiveView.Witness.report_certificate(%{"id" => id, "status" => "ok"})
            LiveView.Witness.verify_ui!(id)
            :ok
          else
            # Explicitly fail if the element is missing from the server-side render
            flunk("LiveView.Witness assertion failed: element #{selector} not found in view")
          end
      end
    end
  end

  @doc """
  Refutes that a UI element is rendered.

  When called with a `Phoenix.LiveView.Socket`, it triggers an expectation
  and synchronously waits for a client certificate, expecting it to report failure.

  When called with a `Phoenix.LiveViewTest.View`, it verifies the element absence
  using `has_element?/2` and simulates an error certificate report.

  ## Examples

      # In a test
      refute_witness_rendered(view, ".project-grid")
  """
  defmacro refute_witness_rendered(view, selector, expectations \\ []) do
    quote bind_quoted: [view: view, selector: selector, expectations: expectations] do
      case view do
        %Phoenix.LiveView.Socket{} = socket ->
          {:ok, id, socket} = LiveView.Witness.expect_ui(socket, selector, expectations)

          # We expect verify_ui! to raise when the client reports an error
          # If it returns :ok, the refutation fails.
          try do
            LiveView.Witness.verify_ui!(id)
            flunk("LiveView.Witness refutation failed: element #{selector} was successfully rendered")
          rescue
            RuntimeError -> :ok
          end
          socket

        %Phoenix.LiveViewTest.View{} = view ->
          # L1 reality check: verify element absence in the HTML
          if has_element?(view, selector) do
            flunk("LiveView.Witness refutation failed: element #{selector} found in view")
          else
            # L2/L3 simulation: start Witness protocol and report an error
            {:ok, id, _} = LiveView.Witness.expect_ui(view, selector, expectations)
            LiveView.Witness.report_certificate(%{"id" => id, "status" => "error", "message" => "Refuted"})

            # verify_ui! should raise an error as reported
            try do
              LiveView.Witness.verify_ui!(id)
              flunk("LiveView.Witness refutation failed: verify_ui! did not raise as expected")
            rescue
              RuntimeError -> :ok
            end
          end
      end
    end
  end
end
