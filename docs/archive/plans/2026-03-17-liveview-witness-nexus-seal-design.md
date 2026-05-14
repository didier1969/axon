# Design: LiveView.Witness - The Nexus Seal (Absolute Truth)

## 🎯 Vision
This final evolution of **LiveView.Witness** transforms it from a high-quality library into an impenetrable armor of reliability. It eliminates technical "lies" caused by timing (Race Conditions), security gaps (Oracle Spoofing), and visibility blind spots (Shadow DOM).

---

## 🏛️ Final Architecture Pillars

### 1. Render Synchronization (The End of Race Conditions)
To ensure the "Source of Truth" is perfectly synchronized with the browser's painting cycle:
*   **DOM Stability Observer:** The JS Hook will use a `MutationObserver` to wait for the target element to appear or update before failing.
*   **Paint Alignment:** Verification is delayed until the next `requestAnimationFrame`, guaranteeing the user is physically seeing what we are checking.
*   **Contract Debouncing:** Contracts are queued and executed only once the DOM has settled for a configurable window (default 100ms).

### 2. The Oracle Shield (Secure OOB Diagnostics)
To prevent malicious or accidental diagnostic spoofing:
*   **Dynamic Security Token:** A unique, cryptographically secure token is generated per application session (BEAM level).
*   **Token Handshake:** The token is injected into the HTML via the Survival Watchdog.
*   **Mandatory Verification:** The `LiveView.Witness.Oracle` Plug strictly enforces the presence of this token in the `X-Witness-Token` header.

### 3. Deep-Sight Inspection (Shadow DOM & Visibility)
To provide 100% coverage of modern web interfaces:
*   **Shadow Root Traversal:** The inspector recursively traverses `.shadowRoot` properties to find elements hidden inside Web Components.
*   **Computed Visibility:** The L2 check is enhanced to handle zero-pixel containers and `clip-path` masks.

---

## 🚀 Final Workflow

1.  **Boot:** Elixir generates a `Witness.Token`.
2.  **Layout:** `{LiveView.Witness.HTML.watchdog_script(token: Witness.Token.get())}` is rendered.
3.  **Action:** Server pushes a contract.
4.  **Wait:** Client Hook observes the DOM. If the element is being patched, it waits.
5.  **Verify:** Once stable, the physical audit (including Shadow DOM) is performed.
6.  **Secure Report:** Health alerts are sent to the Oracle with the mandatory Security Token.

---

## ✅ Final Success Criteria (The Nexus Seal)
*   [ ] Zero "False Negatives" in TDD caused by Phoenix patch timing.
*   [ ] The Oracle endpoint is inaccessible to unauthorized requests.
*   [ ] Web Components (Shadow DOM) are fully transparent to the Witness.
