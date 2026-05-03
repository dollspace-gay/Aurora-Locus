// Fetch wrapper with auth headers + JSON convenience.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §12.3.3: api/client.js absorbs the
// per-fetch boilerplate from the previous script.js. Higher-level
// callers use endpoints.js helpers; capability-routed substrate uses
// api/capabilities.js.

(function (global) {
  'use strict';

  const API_BASE = '/xrpc';

  function authHeaders(extra) {
    const headers = Object.assign({}, extra || {});
    const token = global.AuroraSession ? global.AuroraSession.token() : localStorage.getItem('adminToken');
    if (token) headers.Authorization = 'Bearer ' + token;
    return headers;
  }

  // GET <nsid>?<params>. Returns parsed JSON or throws.
  async function get(nsid, params) {
    const qs = params ? '?' + new URLSearchParams(params).toString() : '';
    const res = await fetch(API_BASE + '/' + nsid + qs, { headers: authHeaders() });
    return await handle(res);
  }

  // POST <nsid> with JSON body.
  async function post(nsid, body, extraHeaders) {
    const res = await fetch(API_BASE + '/' + nsid, {
      method: 'POST',
      headers: authHeaders(Object.assign({ 'Content-Type': 'application/json' }, extraHeaders || {})),
      body: body == null ? undefined : JSON.stringify(body),
    });
    return await handle(res);
  }

  // POST returning the raw Response (for downloads, header inspection).
  async function postRaw(nsid, body, extraHeaders) {
    return await fetch(API_BASE + '/' + nsid, {
      method: 'POST',
      headers: authHeaders(Object.assign({ 'Content-Type': 'application/json' }, extraHeaders || {})),
      body: body == null ? undefined : JSON.stringify(body),
    });
  }

  async function handle(res) {
    if (!res.ok) {
      let detail = '';
      try {
        const body = await res.json();
        detail = body && (body.message || body.error) ? ': ' + (body.message || body.error) : '';
      } catch (e) { /* ignore */ }
      const err = new Error('HTTP ' + res.status + detail);
      err.status = res.status;
      // Auto-logout on 401 for protected endpoints.
      if (res.status === 401 && global.AuroraSession) {
        global.AuroraSession.logout();
      }
      throw err;
    }
    if (res.status === 204) return null;
    return await res.json();
  }

  global.AuroraClient = {
    get: get,
    post: post,
    postRaw: postRaw,
    apiBase: API_BASE,
  };
})(window);
