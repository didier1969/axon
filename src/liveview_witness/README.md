# LiveView.Witness

LiveView.Witness is a "Physical Reality" Inspector for Phoenix LiveView applications. It provides three levels of integrity checks to ensure that what the server thinks it rendered is actually visible and interactive for the user.

## Features

- **L1 (Presence):** Verifies the element or a set of elements exist in the DOM.
- **L2 (Physical Reality):** Verifies elements are not hidden (display: none, visibility: hidden, opacity: 0), not zero-sized, not off-screen, and not occluded by other elements.
- **L3 (Global Health):** Monitors window-level errors and unhandled promise rejections on the client.
- **Out-of-Bound (OOB) Oracle:** A standalone Plug to receive diagnostic alerts even when the Phoenix Socket is disconnected or the server is returning 500s.

## Installation

Add `liveview_witness` to your `mix.exs` dependencies:

```elixir
def deps do
  [
    {:liveview_witness, path: "../liveview_witness"} # Or hex version when available
  ]
end
```

## Setup

### 1. JS Hook Registration

Import and add the `LiveViewWitness` hook to your `LiveSocket`:

```javascript
import LiveViewWitness from "../../deps/liveview_witness/priv/static/liveview_witness.js"

let liveSocket = new LiveSocket("/live", Socket, {
  params: {_csrf_token: csrfToken},
  hooks: { LiveViewWitness, ... }
})
```

### 2. (Optional) Oracle Plug

To handle Out-of-Bound diagnostics (e.g., when the socket is disconnected), add the Oracle Plug to your `endpoint.ex`:

```elixir
plug LiveView.Witness.Oracle
```

This will listen for `POST /liveview_witness/diagnose` and log critical client-side failures.

## Usage

In your LiveView, use the `expect_ui/3` function to push a rendering contract to the client:

```elixir
def mount(_params, _session, socket) do
  if connected?(socket) do
    LiveView.Witness.expect_ui(socket, ".project-card", min_items: 1)
  end
  {:ok, socket}
end
```

Handle the "certificate" (success/failure) or "health_alert" (L3) events in your LiveView:

```elixir
def handle_event("phx-witness:certificate", report, socket) do
  # Log or react to the UI reality check
  {:noreply, socket}
end

def handle_event("phx-witness:health_alert", alert, socket) do
  # Handle JS errors or unhandled rejections
  {:noreply, socket}
end
```

In your template, attach the hook to a container:

```heex
<div id="my-container" phx-hook="LiveViewWitness">
  ...
</div>
```
