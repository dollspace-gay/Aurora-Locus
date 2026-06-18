// Fetch wrapper with auth headers + JSON convenience.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §12.3.3: api/client.js absorbs the
// per-fetch boilerplate from the previous script.js. Higher-level
// callers use endpoints.js helpers; capability-routed substrate uses
// api/capabilities.js.

(function (global) {
  'use strict';

  const API_BASE = '/xrpc';
  const REFRESH_URL = '/admin-oauth/refresh';

  function authHeaders(extra) {
    const headers = Object.assign({}, extra || {});
    const token = global.AuroraSession ? global.AuroraSession.token() : localStorage.getItem('aurora-admin-token');
    if (token) headers.Authorization = 'Bearer ' + token;
    return headers;
  }

  // Silent refresh-on-401 (§8.1.2 / #268). When a request 401s on an
  // expired access token, exchange the stored refresh token for a fresh
  // access token and retry once — the operator never sees an interactive
  // re-auth unless the refresh itself fails (refresh token expired/revoked
  // or no refresh token present), in which case the 401 reaches handle()
  // and bounces to login as before. Reactive-on-401 only; proactive
  // near-expiry refresh is deferred (the 0.9.3 session-management pass).
  let refreshInFlight = null;

  function storedRefreshToken() {
    return global.AuroraSession ? global.AuroraSession.refreshToken()
      : localStorage.getItem('aurora-admin-refresh-token');
  }

  function applyRefreshed(data) {
    if (global.AuroraSession) {
      global.AuroraSession.setToken(data.access_token);
      if (data.refresh_token) global.AuroraSession.setRefreshToken(data.refresh_token);
    } else {
      localStorage.setItem('aurora-admin-token', data.access_token);
      if (data.refresh_token) localStorage.setItem('aurora-admin-refresh-token', data.refresh_token);
    }
  }

  // Single-flight: concurrent 401s share one in-flight refresh so we never
  // fire N parallel refresh calls (which, with rotation, would invalidate
  // each other). Resolves true on success, false otherwise.
  function refreshAccessToken() {
    if (refreshInFlight) return refreshInFlight;
    const rt = storedRefreshToken();
    if (!rt) return Promise.resolve(false);
    refreshInFlight = (async function () {
      try {
        const res = await fetch(REFRESH_URL, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ refresh_token: rt }),
        });
        if (!res.ok) return false;
        const data = await res.json();
        if (!data || !data.access_token) return false;
        applyRefreshed(data);
        return true;
      } catch (e) {
        return false;
      } finally {
        refreshInFlight = null;
      }
    })();
    return refreshInFlight;
  }

  // Run a fetch thunk; on a 401, attempt one silent refresh + retry. The
  // thunk rebuilds auth headers each call, so the retry picks up the new
  // token. A still-401 (or non-401) response is returned as-is to handle().
  async function withRefresh(doFetch) {
    let res = await doFetch();
    if (res.status === 401) {
      const refreshed = await refreshAccessToken();
      if (refreshed) res = await doFetch();
    }
    return res;
  }

  // GET <nsid>?<params>. Returns parsed JSON or throws.
  async function get(nsid, params) {
    let qs = '';
    if (params) {
      // Build the query string explicitly so array values become REPEATED keys
      // (`metrics=a&metrics=b`), not a comma-joined single value. XRPC `query`
      // endpoints with list params (e.g. getModerationMetrics' `metrics`) parse
      // via axum_extra Query, which requires repeated keys; the default
      // `new URLSearchParams(obj)` stringifies an array to one comma-joined
      // value and would 400. Scalars append once; null/undefined are skipped.
      const sp = new URLSearchParams();
      for (const k of Object.keys(params)) {
        const v = params[k];
        if (v == null) continue;
        if (Array.isArray(v)) v.forEach((item) => { if (item != null) sp.append(k, item); });
        else sp.append(k, v);
      }
      const s = sp.toString();
      if (s) qs = '?' + s;
    }
    const res = await withRefresh(() => fetch(API_BASE + '/' + nsid + qs, { headers: authHeaders() }));
    return await handle(res);
  }

  // POST <nsid> with JSON body.
  async function post(nsid, body, extraHeaders) {
    const res = await withRefresh(() => fetch(API_BASE + '/' + nsid, {
      method: 'POST',
      headers: authHeaders(Object.assign({ 'Content-Type': 'application/json' }, extraHeaders || {})),
      body: body == null ? undefined : JSON.stringify(body),
    }));
    return await handle(res);
  }

  // POST returning the raw Response (for downloads, header inspection).
  async function postRaw(nsid, body, extraHeaders) {
    return await withRefresh(() => fetch(API_BASE + '/' + nsid, {
      method: 'POST',
      headers: authHeaders(Object.assign({ 'Content-Type': 'application/json' }, extraHeaders || {})),
      body: body == null ? undefined : JSON.stringify(body),
    }));
  }

  async function handle(res) {
    if (!res.ok) {
      let body = null;
      try { body = await res.json(); } catch (e) { /* non-JSON 4xx/5xx */ }

      const code = body && body.error;
      const serverMessage = body && body.message;
      const failingSubject = body && body.failingSubject;
      const failingSubjectId = body && body.failingSubjectId;

      let displayMessage;
      let detailsAvailable = false;

      // 4xx errors may carry a structured error code with a
      // friendlier prose template in AuroraErrorTranslations.
      // Defensive guard on the module's presence so a script
      // load-order regression falls back silently to raw rendering
      // rather than throwing.
      if (res.status >= 400 && res.status < 500 &&
          global.AuroraErrorTranslations &&
          global.AuroraErrorTranslations.has(code)) {
        displayMessage = global.AuroraErrorTranslations.translate(code);
        detailsAvailable = true;
      } else {
        const detail = serverMessage || code;
        displayMessage = 'HTTP ' + res.status + (detail ? ': ' + detail : '');
      }

      // v0.3 per-subject error envelope: when a batch action aborts
      // atomically on one bad subject, surface which subject inline
      // so the operator knows where to look.
      let subjectContext = '';
      if (failingSubject || failingSubjectId) {
        const parts = [];
        if (failingSubject) parts.push('subject: ' + failingSubject);
        if (failingSubjectId) parts.push('id: ' + failingSubjectId);
        subjectContext = ' (' + parts.join(', ') + ')';
      }

      const err = new Error(displayMessage + subjectContext);
      err.status = res.status;
      err.code = code;
      err.serverMessage = serverMessage;
      err.detailsAvailable = detailsAvailable;
      err.failingSubject = failingSubject;
      err.failingSubjectId = failingSubjectId;
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
