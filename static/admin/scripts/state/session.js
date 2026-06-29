// Operator session state. Per docs/AURORA_ADMIN_UI_DESIGN.md §12.3.3:
// authentication and current-operator state moved from script.js
// globals into an explicit state module with a small subscribe API.
//
// localStorage key: 'aurora-admin-token' (renamed from the legacy
// 'adminToken' per §8.1.1 / recon §4.6). A one-time boot migration below
// moves an existing legacy token to the new key so a session created
// before the rename survives the upgrade. adminDid / adminRole are
// separate keys and out of scope for this rename.

(function (global) {
  'use strict';

  const TOKEN_KEY = 'aurora-admin-token';
  const LEGACY_TOKEN_KEY = 'adminToken';
  // Refresh-token store (§8.1.2 / #268). localStorage was the chosen
  // strategy (see §8.1.2 doc note): same XSS-exfil surface the access
  // token already has, documented as the accepted threat model, no
  // httpOnly-cookie/backend-coordination cost. Consumed by
  // api/client.js's silent refresh-on-401.
  const REFRESH_KEY = 'aurora-admin-refresh-token';

  // One-time token-key migration (§8.1.1). Runs at module load, before any
  // consumer reads token() — session.js loads ahead of client.js,
  // capabilities.js, and app.js in index.html. Read the legacy key, write
  // it under the new key if the new key isn't already set, then drop the
  // legacy key so the rename completes in a single visit.
  (function migrateLegacyToken() {
    try {
      const legacy = localStorage.getItem(LEGACY_TOKEN_KEY);
      if (legacy && !localStorage.getItem(TOKEN_KEY)) {
        localStorage.setItem(TOKEN_KEY, legacy);
      }
      if (legacy != null) localStorage.removeItem(LEGACY_TOKEN_KEY);
      // Same migration for the refresh token (§8.1.2 / #268): an operator
      // who logged in via the legacy OAuth-callback page before the rename
      // has a lingering 'adminRefreshToken'; move it to the canonical key
      // so the new refresh-on-401 consumer finds it.
      const legacyRefresh = localStorage.getItem('adminRefreshToken');
      if (legacyRefresh && !localStorage.getItem(REFRESH_KEY)) {
        localStorage.setItem(REFRESH_KEY, legacyRefresh);
      }
      if (legacyRefresh != null) localStorage.removeItem('adminRefreshToken');
    } catch (e) { /* localStorage unavailable — nothing to migrate */ }
  })();

  let currentUser = null;
  const subscribers = new Set();

  function token() {
    return localStorage.getItem(TOKEN_KEY) || '';
  }

  function setToken(t) {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
    notify();
  }

  function refreshToken() {
    return localStorage.getItem(REFRESH_KEY) || '';
  }

  function setRefreshToken(t) {
    if (t) localStorage.setItem(REFRESH_KEY, t);
    else localStorage.removeItem(REFRESH_KEY);
  }

  function user() {
    return currentUser;
  }

  function setUser(u) {
    currentUser = u;
    notify();
  }

  // Role resolution order:
  //   1. currentUser.role  (live in-memory copy if getSession returned a role)
  //   2. localStorage.adminRole  (set by the OAuth callback in login.js)
  //   3. 'moderator'  (least-privileged fallback)
  // The standard com.atproto.server.getSession lexicon does not include an
  // admin tier, so step 1 is usually undefined and step 2 is the actual
  // source of truth post-OAuth.
  function role() {
    if (currentUser && currentUser.role) return currentUser.role;
    const stored = localStorage.getItem('adminRole');
    if (stored) return stored;
    return 'moderator';
  }

  // Per Section 4.2 — role tier comparison.
  function hasRole(required) {
    const r = role();
    const order = { moderator: 1, admin: 2, superadmin: 3 };
    return (order[r] || 0) >= (order[required] || 0);
  }

  function subscribe(fn) {
    subscribers.add(fn);
    return () => subscribers.delete(fn);
  }

  function notify() {
    for (const fn of subscribers) {
      try { fn({ user: currentUser, token: token() }); } catch (e) { /* ignore */ }
    }
  }

  function logout() {
    setToken('');
    setRefreshToken('');
    setUser(null);
    window.location.href = '/admin/login.html';
  }

  global.AuroraSession = {
    token: token,
    setToken: setToken,
    refreshToken: refreshToken,
    setRefreshToken: setRefreshToken,
    user: user,
    setUser: setUser,
    role: role,
    hasRole: hasRole,
    subscribe: subscribe,
    logout: logout,
  };
})(window);
