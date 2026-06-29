// Filter-state-in-URL substrate (§5.7.5).
//
// List pages encode their filter state in the hash query string, e.g.
//   #mod/events?actor=did:plc:abc&type=takedown
// so a filtered view is deep-linkable, survives a paste/bookmark, and is
// navigable with the browser back/forward buttons.
//
// Mechanism: write() sets `location.hash` to `<current-path>?<query>`,
// which fires `hashchange` → the router re-dispatches → the page
// remounts and reads read() to seed both its FilterStrip and its initial
// fetch. Because each write is an ordinary hash change, back/forward move
// through filter states for free. The router's parseHash strips the
// `?...` before route matching (a route pattern never contains a query),
// so the path still resolves normally.
//
// Scope: FILTER state. Opaque atproto pagination cursors and the per-page
// cursor stack stay in page module state (a filter change resets
// pagination anyway). Cursor-in-URL is a possible later addition.
//
// API:
//   AuroraUrlState.read()          → { actor: 'did:…', type: 'takedown' }
//   AuroraUrlState.write(filters)  → rewrites the hash query, preserving route
//   AuroraUrlState.clear()         → drops the query, preserving route

(function (global) {
  'use strict';

  function rawHash() {
    return window.location.hash || '';
  }

  // The route path portion of the current hash — everything after the
  // leading '#'/'#/' and before any '?'. Matches the router's parseHash.
  function pathPart() {
    const h = rawHash().replace(/^#\/?/, '');
    const q = h.indexOf('?');
    return q >= 0 ? h.slice(0, q) : h;
  }

  // Parse the current hash query string into a plain object. Values are
  // strings; a consumer that stored a structured value (e.g. a date
  // range) JSON-encodes it on write and decodes on read.
  function read() {
    const h = rawHash();
    const q = h.indexOf('?');
    const out = {};
    if (q < 0) return out;
    try {
      const params = new URLSearchParams(h.slice(q + 1));
      params.forEach((v, k) => { out[k] = v; });
    } catch (e) { /* malformed query → empty filter set */ }
    return out;
  }

  // Serialize a filter object to a query string, dropping empty / false /
  // null values so a "no filters" state produces a clean, query-less URL.
  function serialize(filters) {
    const params = new URLSearchParams();
    const keys = Object.keys(filters || {});
    for (const k of keys) {
      const v = filters[k];
      if (v == null || v === '' || v === false) continue;
      params.set(k, (typeof v === 'object') ? JSON.stringify(v) : String(v));
    }
    return params.toString();
  }

  // Compose the target hash for the current route + the given filters.
  function targetFor(filters) {
    const qs = serialize(filters);
    return '#' + pathPart() + (qs ? '?' + qs : '');
  }

  function write(filters) {
    const target = targetFor(filters);
    // No-op if nothing changed, so a redundant apply doesn't churn history
    // or trigger a pointless remount.
    const current = '#' + rawHash().replace(/^#\/?/, '');
    if (current === target) return;
    window.location.hash = target;
  }

  function clear() {
    const target = '#' + pathPart();
    const current = '#' + rawHash().replace(/^#\/?/, '');
    if (current === target) return;
    window.location.hash = target;
  }

  // Like write(), but updates the URL via history.replaceState — no
  // hashchange fires, so the page does NOT remount. For live-search
  // consumers (e.g. Accounts) where the remount write() triggers would
  // destroy the focused input mid-typing. Trade-off: no per-change history
  // entry, so back/forward won't step through incremental search states —
  // which is the right behaviour for as-you-type search.
  function replace(filters) {
    const target = targetFor(filters);
    const current = '#' + rawHash().replace(/^#\/?/, '');
    if (current === target) return;
    try {
      window.history.replaceState(null, '', target);
    } catch (e) {
      // Older browsers without replaceState — fall back to a hash set
      // (this does remount, but it's a rare path).
      window.location.hash = target.slice(1);
    }
  }

  global.AuroraUrlState = { read: read, write: write, replace: replace, clear: clear };
})(window);
