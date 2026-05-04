// Subscription substrate (substrate primitive 18) +
// Real-time indicator (substrate primitive 19).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §6.18 + §6.19. v0.2 ships the
// substrate as HTTP polling against the existing read endpoints
// (queryEvents). Browsers cannot send custom Authorization headers on
// WebSocket connections — only Sec-WebSocket-Protocol — so the
// previously-coded WebSocket path could never authenticate against
// AdminAuthContext, leaving the indicator stuck in 'reconnecting'.
// Polling sidesteps that, uses the same Bearer-token machinery as
// every other admin call, and lets the indicator reach a stable
// 'polling' state.
//
// The external API is preserved so consumer pages don't change:
//
//   const sub = AuroraSubscription.subscribe(feature, filters, handlers)
//   sub.unsubscribe()
//
// `feature` is a high-level capability name routed via
// AuroraCapabilities (substrate primitive 21). v0.2 ships
// 'subscribe-mod-events' only (other features extend the map without
// component changes).
//
// Real-time indicator states (per §6.19):
//   'connecting'  — initial; one tick before the first poll resolves
//   'polling'     — at least one poll has succeeded; live tail active
//   'offline'     — repeated poll failures or explicit unsubscribe
//
// Backoff on errors: 1× → 2× → 4× → 8× the base interval. Resets on
// success.
//
// Vanilla JS, no framework dependencies (per §12.9).

(function (global) {
  'use strict';

  // Map from logical feature name to the read endpoint used to drive
  // the polling tail. The endpoint must accept query-string filters
  // and return `{items: [{id, ...}]}` ordered newest-first.
  const FEATURE_ENDPOINTS = {
    'subscribe-mod-events': function (filters) {
      return global.AuroraEndpoints.moderator.queryEvents(
        Object.assign({ limit: 25 }, filters || {})
      );
    },
  };

  const POLL_INTERVAL_MS = 10000;
  const MAX_BACKOFF_MS = 80000;
  // Allow a couple of consecutive failures before flipping the
  // indicator to 'offline' — flaky single requests shouldn't spam
  // state transitions.
  const FAILURES_BEFORE_OFFLINE = 3;

  function Subscription(feature, filters, handlers) {
    this.feature = feature;
    this.filters = Object.assign({}, filters || {});
    this.handlers = handlers || {};
    this.disposed = false;
    this.indicators = new Set();
    this.lastSeenId = null;
    this.consecutiveFailures = 0;
    this.intervalMs = POLL_INTERVAL_MS;
    this.timer = null;
    this._setState('connecting');
    // Kick off the first poll immediately so the indicator can leave
    // 'connecting' on the next tick rather than after the full
    // interval.
    this._tick();
  }

  Subscription.prototype._scheduleNext = function (delay) {
    if (this.disposed) return;
    this.timer = setTimeout(() => this._tick(), delay);
  };

  Subscription.prototype._tick = async function () {
    if (this.disposed) return;
    const fetchFn = FEATURE_ENDPOINTS[this.feature];
    if (!fetchFn) {
      this._setState('offline');
      if (this.handlers.onError) this.handlers.onError({ code: 'UnknownFeature' });
      return;
    }
    try {
      const data = await fetchFn(this.filters);
      this._onPollSuccess(data);
    } catch (e) {
      this._onPollFailure(e);
    }
    this._scheduleNext(this.intervalMs);
  };

  Subscription.prototype._onPollSuccess = function (data) {
    const wasOffline = this.state !== 'polling';
    this.consecutiveFailures = 0;
    this.intervalMs = POLL_INTERVAL_MS;
    const items = (data && Array.isArray(data.items)) ? data.items : [];
    if (this.lastSeenId == null) {
      // First successful poll establishes the watermark. Emit a
      // synthesized 'hello' so consumers that care can mirror the
      // previous WebSocket bootstrap shape.
      this.lastSeenId = items.length > 0 ? items[0].id : 0;
      if (this.handlers.onHello) {
        this.handlers.onHello({ sequence: this.lastSeenId });
      }
    } else {
      // Items are newest-first. Emit only those strictly newer than
      // the last seen id, in chronological (id-ascending) order so
      // consumers see them as they would on a live stream.
      const fresh = items.filter((it) => it && typeof it.id === 'number' && it.id > this.lastSeenId);
      fresh.sort((a, b) => a.id - b.id);
      for (const evt of fresh) {
        if (this.handlers.onEvent) this.handlers.onEvent(evt, evt.id);
        this.lastSeenId = evt.id;
      }
    }
    this._setState('polling');
    if (wasOffline && this.handlers.onConnected) this.handlers.onConnected();
  };

  Subscription.prototype._onPollFailure = function (err) {
    this.consecutiveFailures += 1;
    if (this.handlers.onError) {
      this.handlers.onError({ code: 'PollFailure', message: err && err.message });
    }
    if (this.consecutiveFailures >= FAILURES_BEFORE_OFFLINE) {
      const wasPolling = this.state === 'polling';
      this._setState('offline');
      if (wasPolling && this.handlers.onDisconnected) this.handlers.onDisconnected();
      // Exponential backoff on persistent failure to avoid hammering
      // a degraded server.
      this.intervalMs = Math.min(this.intervalMs * 2, MAX_BACKOFF_MS);
    }
  };

  Subscription.prototype.unsubscribe = function () {
    this.disposed = true;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
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
      connecting: { dot: 'rt-dot-reconnecting', label: 'Connecting…', live: 'Connecting' },
      polling: { dot: 'rt-dot-polling', label: 'Live (polling)', live: 'Live event tail active' },
      offline: { dot: 'rt-dot-offline', label: 'Offline', live: 'Disconnected' },
    };
    const v = variants[state] || variants.offline;
    el.classList.add('rt-indicator');
    el.setAttribute('aria-live', 'polite');
    el.innerHTML = '<span class="rt-dot ' + v.dot + '" aria-hidden="true"></span>' +
                   '<span class="rt-label">' + v.label + '</span>';
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
