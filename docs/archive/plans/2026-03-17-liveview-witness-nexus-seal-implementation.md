# LiveView.Witness - The Nexus Seal Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Achieve 100% technical truth by implementing Render Synchronization, Oracle Security, and Shadow DOM support.

**Architecture:** 
1.  **Sync:** `MutationObserver` + `requestAnimationFrame` in JS.
2.  **Security:** Dynamic session token generated in Elixir and verified in Plug.
3.  **Depth:** Recursive shadow root traversal in JS.

---

### Task 1: The Oracle Shield (Elixir Token Security)

**Files:**
- Create: `src/liveview_witness/lib/liveview_witness/token.ex`
- Modify: `src/liveview_witness/lib/liveview_witness/application.ex`
- Modify: `src/liveview_witness/lib/liveview_witness/oracle.ex`
- Modify: `src/liveview_witness/lib/liveview_witness/html.ex`

**Step 1: Implement Token Store**

Create a simple `Agent` or `ETS` based store to hold the session token.

**Step 2: Update Application**

Start the Token store in the supervision tree. Generate a random 32-char hex token on boot.

**Step 3: Secure the Oracle Plug**

Update `Oracle.call/2` to check the `X-Witness-Token` header. Return 401 Unauthorized if missing or invalid.

**Step 4: Update HTML Helper**

Update `watchdog_script/1` to inject the token from the store into the JS `fetch` headers.

---

### Task 2: Render Synchronization (JS Stability)

**Files:**
- Modify: `src/liveview_witness/priv/static/liveview_witness.js`

**Step 1: Implement `waitUntilStable` helper**

Wrap the inspection in a function that uses `MutationObserver`. It should resolve when the element matches the selector OR when a timeout occurs.

**Step 2: Paint Alignment**

Use `requestAnimationFrame` to ensure the final check happens after the browser has finished painting the last patch.

**Step 3: Update `handleEvent`**

Modify the contract handler to use this async stability logic before pushing the certificate back.

---

### Task 3: Deep-Sight Inspection (Shadow DOM)

**Files:**
- Modify: `src/liveview_witness/priv/static/liveview_witness.js`

**Step 1: Implement `deepQuerySelectorAll`**

A recursive function that searches through `shadowRoot` of every element to find matches for the selector.

**Step 2: Update `inspect` logic**

Use `deepQuerySelectorAll` instead of `document.querySelectorAll`.

---

### Task 4: Final Integration & Verification

**Files:**
- Modify: `src/dashboard/lib/axon_dashboard_web/components/layouts/root.html.heex`

**Step 1: Verify Oracle Security**

Manually try to curl the diagnostic endpoint without a token. Verify it fails.

**Step 2: Verify Sync Stability**

Simulate a slow-rendering component and verify that Witness waits for it before reporting.

**Step 3: Commit**

`feat: finalize LiveView.Witness with Nexus Seal (Sync, Security, Shadow DOM)`.
