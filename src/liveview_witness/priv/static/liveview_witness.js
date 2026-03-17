/**
 * LiveView.Witness JS Inspector Hook (L1/L2/L3)
 * Measures the physical reality of the DOM.
 */
const LiveViewWitness = {
  mounted() {
    this.handleEvent("phx-witness:contract", (payload) => {
      const report = this.inspect(payload);
      this.pushEvent("phx-witness:certificate", report);
    });

    // L3: Global health monitoring
    this._onWindowError = (event) => {
      this.pushEvent("phx-witness:health_alert", {
        type: "error",
        message: event.message,
        source: event.filename,
        lineno: event.lineno,
        colno: event.colno
      });
    };

    this._onWindowRejection = (event) => {
      this.pushEvent("phx-witness:health_alert", {
        type: "unhandledrejection",
        reason: event.reason ? (event.reason.message || event.reason) : "unknown"
      });
    };

    window.addEventListener("error", this._onWindowError);
    window.addEventListener("unhandledrejection", this._onWindowRejection);
  },

  destroyed() {
    window.removeEventListener("error", this._onWindowError);
    window.removeEventListener("unhandledrejection", this._onWindowRejection);
  },

  inspect(_payload) {
    const el = this.el;

    // L1: Presence
    if (!el || !document.contains(el)) {
      return { 
        status: "error", 
        level: "L1", 
        message: "Element not in DOM"
      };
    }

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
      message: "Reality confirmed"
    };
  }
};

export default LiveViewWitness;
