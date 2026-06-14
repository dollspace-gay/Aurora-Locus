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
    const token = global.AuroraSession ? global.AuroraSession.token() : localStorage.getItem('aurora-admin-token');
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
