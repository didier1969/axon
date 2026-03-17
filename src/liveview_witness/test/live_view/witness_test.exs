defmodule LiveView.WitnessTest do
  use ExUnit.Case, async: true
  import Phoenix.LiveView.Test

  test "expect_ui/3 pushes phx-witness:contract event" do
    # Create a mock socket
    socket = %Phoenix.LiveView.Socket{}
    
    # Call expect_ui
    socket = LiveView.Witness.expect_ui(socket, ".project-grid", [is_visible: true])
    
    # In Phoenix.LiveView.Socket, pushed events are stored in :push_events
    # However, testing push_event is usually done via render_click/render_submit
    # but since this is a unit test for a helper, we can check the internal state
    # if we know where it's stored, or just use Phoenix.LiveView.Test.assert_push_event
    # when using live_view test.
    
    # Since we are testing a simple helper that calls Phoenix.LiveView.push_event,
    # and we are in a unit test, we can check the socket's internal state.
    
    assert [%{event: "phx-witness:contract", payload: payload}] = socket.endpoint.__events__.(socket)
    assert payload.selector == ".project-grid"
    assert payload.expectations == %{is_visible: true}
    assert byte_size(payload.id) == 16 # Base16 of 8 bytes
  end
end
