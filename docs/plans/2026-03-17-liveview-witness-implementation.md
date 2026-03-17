# LiveView.Witness Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a standalone Elixir library for real-time UI verification (L1/L2/L3) and OOB diagnostics in Phoenix LiveView.

**Architecture:** A three-part system consisting of an OOB Plug (The Oracle), a JS Hook (The Inspector), and an Elixir API (The Contract) to ensure the "Source of Truth" from browser to server.

**Tech Stack:** Elixir, Phoenix, JavaScript (Vanilla), Plug.

---

### Task 1: Project Initialization

**Files:**
- Create: `src/liveview_witness/mix.exs`
- Create: `src/liveview_witness/lib/liveview_witness.ex`

**Step 1: Initialize the new library project**

Run: `cd src && mix new liveview_witness --module LiveView.Witness`

**Step 2: Update `mix.exs` dependencies**

```elixir
defp deps do
  [
    {:phoenix_live_view, "~> 1.0"},
    {:jason, "~> 1.2"},
    {:plug_cowboy, "~> 2.5"}
  ]
end
```

**Step 3: Commit**

```bash
git add src/liveview_witness
git commit -m "chore: initialize liveview_witness library"
```

---

### Task 2: The Oracle (OOB Plug)

**Files:**
- Create: `src/liveview_witness/lib/liveview_witness/oracle.ex`

**Step 1: Implement the OOB Diagnostic Plug**

```elixir
defmodule LiveView.Witness.Oracle do
  @moduledoc "Handles Out-of-Bound diagnostics (500s, Disconnects)."
  import Plug.Conn

  def init(opts), do: opts

  def call(conn, _opts) do
    if conn.path_info == ["liveview_witness", "diagnose"] do
      {:ok, body, _conn} = read_body(conn)
      # In a real library, we would broadcast this or log it
      IO.warn("[LiveView.Witness] OOB Alert Received: #{body}")
      
      conn
      |> put_resp_content_type("application/json")
      |> send_resp(200, Jason.encode!(%{status: "received"}))
      |> halt()
    else
      conn
    end
  end
end
```

**Step 2: Commit**

```bash
git add src/liveview_witness/lib/liveview_witness/oracle.ex
git commit -m "feat: add OOB Oracle Plug for diagnostics"
```

---

### Task 3: The Inspector (JS Hook)

**Files:**
- Create: `src/liveview_witness/priv/static/liveview_witness.js`

**Step 1: Implement the JS Inspection Logic (L1/L2/L3)**

```javascript
const LiveViewWitness = {
  mounted() {
    this.handleEvent("phx-witness:contract", (payload) => {
      const { id, selector, expectations } = payload;
      const element = document.querySelector(selector);
      
      let report = { id: id, status: "ok", details: {} };
      
      if (!element) {
        report.status = "error";
        report.details.reason = "Element not found in DOM";
      } else {
        // L2: Physical Visibility Check
        const style = window.getComputedStyle(element);
        const isVisible = style.display !== 'none' && 
                          style.visibility !== 'hidden' && 
                          style.opacity !== '0';
        
        // Occlusion check
        const rect = element.getBoundingClientRect();
        const centerX = rect.left + rect.width / 2;
        const centerY = rect.top + rect.height / 2;
        const elementAtPoint = document.elementFromPoint(centerX, centerY);
        const isOccluded = elementAtPoint && !element.contains(elementAtPoint);

        if (!isVisible || isOccluded) {
          report.status = "error";
          report.details.reason = "Element is present but physically hidden or occluded";
        }
      }
      
      this.pushEvent("phx-witness:certificate", report);
    });

    // L3: Console Error Interceptor
    window.addEventListener("error", (e) => {
      this.pushEvent("phx-witness:health_alert", {
        type: "js_error",
        message: e.message,
        stack: e.error ? e.error.stack : null
      });
    });
  }
};

export default LiveViewWitness;
```

**Step 2: Commit**

```bash
git add src/liveview_witness/priv/static/liveview_witness.js
git commit -m "feat: add JS Inspector Hook with L2 visibility and L3 health checks"
```

---

### Task 4: The Elixir Contract API

**Files:**
- Modify: `src/liveview_witness/lib/liveview_witness.ex`

**Step 1: Implement the `expect_ui` and `handle_event` logic**

```elixir
defmodule LiveView.Witness do
  @doc "Pushes a rendering contract to the client."
  def expect_ui(socket, selector, expectations \\ []) do
    contract = %{
      id: UUID.uuid4(),
      selector: selector,
      expectations: Map.new(expectations)
    }
    Phoenix.LiveView.push_event(socket, "phx-witness:contract", contract)
  end
end
```

**Step 2: Commit**

```bash
git add src/liveview_witness/lib/liveview_witness.ex
git commit -m "feat: implement Elixir Contract API"
```

---

### Task 5: Pilot Integration in Axon Dashboard

**Files:**
- Modify: `src/dashboard/mix.exs`
- Modify: `src/dashboard/lib/axon_dashboard_web/live/status_live.ex`
- Modify: `src/dashboard/assets/js/app.js`

**Step 1: Link the local library in Axon Dashboard**

```elixir
# src/dashboard/mix.exs
{:liveview_witness, path: "../liveview_witness"}
```

**Step 2: Register the Hook in `app.js`**

**Step 3: Use Witness to verify the "Project Grid" in `StatusLive`**

Replace the fake multiplier with a Witness Contract that validates the actual count of `.project-card` elements in the browser.

**Step 4: Commit**

```bash
git add src/dashboard
git commit -m "feat: integrate LiveView.Witness pilot into Axon Dashboard"
```
