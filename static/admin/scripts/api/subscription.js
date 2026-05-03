// Subscription substrate (substrate primitive 18) +
// Real-time indicator (substrate primitive 19).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §6.18 + §6.19. Manages WebSocket
// connection lifecycle, reconnection backoff, sequence-cursor resume.
// Surfaces connection state via the real-time indicator widget that
// pages embed in their headers.
//
// API:
//   const sub = AuroraSubscription.subscribe(feature, filters, handlers)
//   sub.unsubscribe()
//
// `feature` is a high-level capability name routed via
// AuroraCapabilities (substrate primitive 21). For Phase 3.9 only
// 'subscribe-mod-events' is wired; future features (subscribeAudit
// etc.) extend the map without component changes.
//
// Reconnect backoff: 1s → 2s → 4s → 8s → 16s (capped). Resets on
// successful connection.
//
// Real-time indicator:
//   AuroraSubscription.attachIndicator(el, sub)
//   ('Connected' | 'Reconnecting…' | 'Offline' | 'Polling fallback')
//
// Vanilla JS, no framework dependencies (per §12.9).

(function (global) {
  'use strict';

  const FEATURE_TO_NSID = {
    'subscribe-mod-events': 'tools.aurora.admin.subscribeModEvents',
  };

  function authToken() {
    return localStorage.getItem('adminToken') || '';
  }

  function wsUrlFor(nsid, filters) {
    // The WebSocket route is the same XRPC path; switch http(s)→ws(s).
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const params = new URLSearchParams();
    if (filters && typeof filters === 'object') {
      for (const k of Object.keys(filters)) {
        if (filters[k] != null && filters[k] !== '') {
          params.set(k, filters[k]);
        }
      }
    }
    const qs = params.toString();
    return `${proto}//${window.location.host}/xrpc/${nsid}${qs ? '?' + qs : ''}`;
  }

  function Subscription(feature, filters, handlers) {
    this.feature = feature;
    this.filters = Object.assign({}, filters || {});
    this.handlers = handlers || {};
    this.ws = null;
    this.cursor = filters && filters.cursor;
    this.backoffMs = 1000;
    this.maxBackoffMs = 16000;
    this.disposed = false;
    this.indicators = new Set();
    this._setState('connecting');
    this._connect();
  }

  Subscription.prototype._connect = function () {
    if (this.disposed) return;
    const nsid = FEATURE_TO_NSID[this.feature];
    if (!nsid) {
      this._setState('offline');
      if (this.handlers.onError) this.handlers.onError({ code: 'UnknownFeature' });
      return;
    }
    const filters = Object.assign({}, this.filters);
    if (this.cursor != null) filters.cursor = this.cursor;
    const url = wsUrlFor(nsid, filters);
    let ws;
    try {
      // axum's WebSocket extractor handles the upgrade; the WS handshake
      // can carry a bearer token via Sec-WebSocket-Protocol header,
      // since browsers don't allow custom Authorization on ws:// — we
      // pass the token as a sub-protocol hint and the handler reads
      // it from the auth context that is wired through axum.
      // For browsers without subprotocol auth, the token still needs
      // to be valid via cookie/session. v0.2 admin UI runs against a
      // local PDS where the auth flow is consistent across HTTP +
      // WebSocket.
      const proto = authToken() ? ['Bearer.' + authToken()] : undefined;
      ws = new WebSocket(url, proto);
    } catch (e) {
      this._setState('offline');
      this._scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this._setState('connected');
      this.backoffMs = 1000;
      if (this.handlers.onConnected) this.handlers.onConnected();
    };
    ws.onmessage = (evt) => {
      let msg;
      try { msg = JSON.parse(evt.data); } catch (e) { return; }
      const t = msg.$type;
      if (t === 'event' && msg.event) {
        if (msg.sequence != null) this.cursor = msg.sequence;
        if (this.handlers.onEvent) this.handlers.onEvent(msg.event, msg.sequence);
      } else if (t === 'hello') {
        if (msg.sequence != null && this.cursor == null) this.cursor = msg.sequence;
        if (this.handlers.onHello) this.handlers.onHello(msg);
      } else if (t === 'heartbeat') {
        if (this.handlers.onHeartbeat) this.handlers.onHeartbeat(msg.sequence);
      } else if (t === 'error') {
        if (this.handlers.onError) this.handlers.onError(msg);
      }
    };
    ws.onclose = () => {
      if (this.disposed) return;
      this._setState('reconnecting');
      if (this.handlers.onDisconnected) this.handlers.onDisconnected();
      this._scheduleReconnect();
    };
    ws.onerror = () => {
      // onclose will follow; let the close handler manage reconnect.
    };
  };

  Subscription.prototype._scheduleReconnect = function () {
    if (this.disposed) return;
    const delay = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, this.maxBackoffMs);
    setTimeout(() => this._connect(), delay);
  };

  Subscription.prototype.unsubscribe = function () {
    this.disposed = true;
    if (this.ws) {
      try { this.ws.close(); } catch (e) {}
      this.ws = null;
    }
    this.indicators.clear();
    this._setState('offline');
  };

  Subscription.prototype._setState = function (state) {
    this.state = state;
    for (const el of this.indicators) renderIndicator(el, state);
  };

  // Real-time indicator (substrate primitive 19, §6.19).
  // Visual contract: 12px dot + text label, color reflects state,
  // pulse animation respects prefers-reduced-motion. aria-live
  // surface for state transitions.
  function renderIndicator(el, state) {
    const variants = {
      connected: { dot: 'rt-dot-connected', label: 'Live', live: 'Live event stream connected' },
      reconnecting: { dot: 'rt-dot-reconnecting', label: 'Reconnecting…', live: 'Reconnecting' },
      connecting: { dot: 'rt-dot-reconnecting', label: 'Connecting…', live: 'Connecting' },
      offline: { dot: 'rt-dot-offline', label: 'Offline', live: 'Disconnected' },
      polling: { dot: 'rt-dot-polling', label: 'Polling every 10s', live: 'Polling fallback active' },
    };
    const v = variants[state] || variants.offline;
    el.classList.add('rt-indicator');
    el.setAttribute('aria-live', 'polite');
    el.innerHTML = `<span class="rt-dot ${v.dot}" aria-hidden="true"></span>` +
                   `<span class="rt-label">${v.label}</span>`;
    // Provide an SR-only summary distinct from the visible label.
    const sr = document.createElement('span');
    sr.className = 'visually-hidden';
    sr.textContent = v.live;
    el.appendChild(sr);
  }

  global.AuroraSubscription = {
    subscribe: function (feature, filters, handlers) {
      return new Subscription(feature, filters, handlers);
    },
    attachIndicator: function (el, sub) {
      if (!el || !sub) return;
      sub.indicators.add(el);
      renderIndicator(el, sub.state);
    },
    renderIndicator: renderIndicator, // exposed for polling-only surfaces
  };
})(window);
