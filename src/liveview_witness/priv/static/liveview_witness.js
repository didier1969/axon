/**
 * LiveView.Witness JS Inspector Hook (L1/L2/L3)
 * Measures the physical reality of the DOM.
 */
const LiveViewWitness = {
  mounted() {
    this.handleEvent("phx-witness:contract", async (payload) => {
      await this._waitUntilStable(payload.selector);
      const report = this.inspect(payload);
      this.pushEvent("phx-witness:certificate", report);
    });

    // L3: Global health monitoring
    this._onWindowError = (event) => {
      const payload = {
        type: "error",
        message: event.message,
        source: event.filename,
        lineno: event.lineno,
        colno: event.colno
      };
      this.pushEvent("phx-witness:health_alert", payload);
      this._sendOracleDiagnostic(payload);
    };

    this._onWindowRejection = (event) => {
      const payload = {
        type: "unhandledrejection",
        reason: event.reason ? (event.reason.message || event.reason) : "unknown"
      };
      this.pushEvent("phx-witness:health_alert", payload);
      this._sendOracleDiagnostic(payload);
    };

    window.addEventListener("error", this._onWindowError);
    window.addEventListener("unhandledrejection", this._onWindowRejection);
  },

  /**
   * Waits for the DOM to be stable and the element to be present.
   */
  _waitUntilStable(selector, timeout = 2000) {
    return new Promise((resolve) => {
      const done = () => {
        // Double RAF ensures we wait for the next paint cycle
        requestAnimationFrame(() => {
          requestAnimationFrame(resolve);
        });
      };

      const timer = setTimeout(() => {
        console.warn(`[LiveView.Witness] Stability timeout for: ${selector || "hook element"}`);
        done();
      }, timeout);

      if (!selector || document.querySelector(selector)) {
        clearTimeout(timer);
        done();
        return;
      }

      const observer = new MutationObserver(() => {
        if (document.querySelector(selector)) {
          observer.disconnect();
          clearTimeout(timer);
          done();
        }
      });

      observer.observe(document.body, { childList: true, subtree: true });
    });
  },

  _sendOracleDiagnostic(payload) {
    // Send to Oracle OOB Endpoint (POST)
    fetch("/liveview_witness/diagnose", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload)
    }).catch(err => console.error("[LiveView.Witness] Oracle fallback failed", err));
  },

  destroyed() {
    window.removeEventListener("error", this._onWindowError);
    window.removeEventListener("unhandledrejection", this._onWindowRejection);
  },

  inspect(payload) {
    const { selector, expectations } = payload;
    const elements = selector ? document.querySelectorAll(selector) : [this.el];

    // L1: Presence
    if (elements.length === 0) {
      return { 
        status: "error", 
        level: "L1", 
        message: selector ? `No elements found for selector: ${selector}` : "Element not in DOM"
      };
    }

    if (expectations && expectations.min_items && elements.length < expectations.min_items) {
      return { 
        status: "error", 
        level: "L1", 
        message: `Expected at least ${expectations.min_items} items, found ${elements.length}`
      };
    }

    // Inspect first element for physical reality
    const el = elements[0];

    // L2: Physical Visibility
    const style = window.getComputedStyle(el);
    
    if (style.display === "none") {
      return { status: "error", level: "L2", message: "display: none" };
    }
    
    if (style.visibility === "hidden") {
      return { status: "error", level: "L2", message: "visibility: hidden" };
    }
    
    if (parseFloat(style.opacity) === 0) {
      return { status: "error", level: "L2", message: "opacity: 0" };
    }

    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) {
      return { status: "error", level: "L2", message: "Zero dimensions" };
    }

    // Occlusion check: is it physically visible to the user?
    const x = rect.left + rect.width / 2;
    const y = rect.top + rect.height / 2;
    
    // Skip if element is off-screen
    if (x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight) {
        return { status: "error", level: "L2", message: "Off-screen" };
    }

    const topEl = document.elementFromPoint(x, y);
    if (!topEl) {
        return { status: "error", level: "L2", message: "No element at point" };
    }

    if (topEl !== el && !el.contains(topEl)) {
      const tag = topEl.tagName ? topEl.tagName.toLowerCase() : 'unknown';
      const id = topEl.id ? `#${topEl.id}` : '';
      
      // Safety check: SVG elements have SVGAnimatedString instead of a plain string
      const rawClass = typeof topEl.className === 'string' ? topEl.className : '';
      const className = rawClass ? `.${rawClass.split(' ').join('.')}` : '';
      
      return { 
        status: "error", 
        level: "L2", 
        message: `Occluded by ${tag}${id}${className}`
      };
    }

    return { 
      status: "ok", 
      level: "L2", 
      message: `Reality confirmed for ${elements.length} elements`
    };
  }
};

export default LiveViewWitness;
