// Operator session state. Per docs/AURORA_ADMIN_UI_DESIGN.md §12.3.3:
// authentication and current-operator state moved from script.js
// globals into an explicit state module with a small subscribe API.
//
// localStorage key: 'adminToken' (preserved from current build for
// backward compat per §12.8); future migration to 'aurora-admin-token'
// happens at app load time.

(function (global) {
  'use strict';

  const TOKEN_KEY = 'adminToken';
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
    setUser(null);
    window.location.href = '/admin/login.html';
  }

  global.AuroraSession = {
    token: token,
    setToken: setToken,
    user: user,
    setUser: setUser,
    role: role,
    hasRole: hasRole,
    subscribe: subscribe,
    logout: logout,
  };
})(window);
