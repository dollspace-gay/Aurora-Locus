// Hash-based router with deep-link handling and legacy-redirect map.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §4.3 + §12.8.
//
// Lifecycle hooks:
//   - register(name, { mount(params, ctx), unmount() })
//   - navigate(hashPath)
//   - currentRoute() returns { pattern, page, params }
//
// Pages register themselves on load. The router watches hashchange,
// resolves the hash against the route table, applies any legacy
// redirect rewrite, gates by required role (per AuroraSession), and
// invokes the matched page's mount() with parameters.

(function (global) {
  'use strict';

  const pages = new Map();
  let currentMounted = null; // { name, instance }
  let mainEl = null;

  function register(name, page) {
    pages.set(name, page);
  }

  function setMain(el) {
    mainEl = el;
  }

  function parseHash() {
    const hash = (window.location.hash || '').replace(/^#\/?/, '');
    return hash;
  }

  // Match a hash path against the route table. Returns
  // { route, params } or null if no match.
  function resolve(hashPath) {
    if (!global.AuroraRoutes) return null;
    const rules = global.AuroraRoutes.routes;
    for (const r of rules) {
      const params = matchPattern(r.pattern, hashPath);
      if (params) return { route: r, params: params };
    }
    return null;
  }

  function matchPattern(pattern, hashPath) {
    const patSegs = pattern.split('/').filter((s) => s !== '');
    const hashSegs = hashPath.split('/').filter((s) => s !== '');
    if (pattern === '' && hashPath === '') return {};
    // Allow :rest to consume the remainder.
    const params = {};
    let i = 0;
    while (i < patSegs.length) {
      const ps = patSegs[i];
      const hs = hashSegs[i];
      if (ps.startsWith(':')) {
        const key = ps.slice(1);
        if (key === 'rest') {
          params[key] = hashSegs.slice(i).join('/');
          return params;
        }
        if (hs == null) return null;
        params[key] = decodeURIComponent(hs);
      } else {
        if (ps !== hs) return null;
      }
      i++;
    }
    if (i !== hashSegs.length) return null;
    return params;
  }

  function applyLegacyRedirect(hashPath) {
    if (!global.AuroraRoutes) return hashPath;
    const map = global.AuroraRoutes.legacyRedirects;
    if (map && map[hashPath]) {
      const target = map[hashPath];
      // Replace, don't push; legacy redirect shouldn't add history entry.
      window.location.replace('#' + target);
      return target;
    }
    return hashPath;
  }

  async function dispatch() {
    const raw = parseHash();
    const hashPath = applyLegacyRedirect(raw);
    const match = resolve(hashPath);
    if (!match) {
      mountNotFound(hashPath);
      return;
    }
    if (match.route.requires && global.AuroraSession) {
      if (!global.AuroraSession.hasRole(match.route.requires)) {
        mountForbidden(match.route);
        return;
      }
    }
    const page = pages.get(match.route.page);
    if (!page || typeof page.mount !== 'function') {
      mountNotFound(hashPath, 'Page not implemented: ' + match.route.page);
      return;
    }
    // Tear down previous page.
    if (currentMounted && currentMounted.instance && typeof currentMounted.instance.unmount === 'function') {
      try { currentMounted.instance.unmount(); } catch (e) { /* ignore */ }
    }
    if (mainEl) mainEl.innerHTML = '';
    try {
      const instance = await Promise.resolve(page.mount({
        params: match.params,
        container: mainEl,
        route: match.route,
      })) || {};
      currentMounted = { name: match.route.page, instance: instance };
    } catch (e) {
      console.error('Page mount error:', e);
      mountError(e);
    }
    updateSidebarActive(hashPath);
    // Aria-live announcement of route change.
    if (global.AuroraA11y) {
      const label = (match.route.page || 'Page').replace(/([A-Z])/g, ' $1').toLowerCase().trim();
      global.AuroraA11y.announce('Navigated to ' + label);
    }
  }

  function mountNotFound(path, msg) {
    if (!mainEl) return;
    mainEl.innerHTML =
      '<header class="page-header"><h2>Page not found</h2></header>' +
      '<p class="empty-state">' + (msg || 'No route matches: <code>#' + (path || '') + '</code>') + '</p>';
  }

  function mountForbidden(route) {
    if (!mainEl) return;
    mainEl.innerHTML =
      '<header class="page-header"><h2>Access denied</h2></header>' +
      '<p class="empty-state">This page requires the <strong>' +
      (route.requires || 'higher') + '</strong> role.</p>';
  }

  function mountError(err) {
    if (!mainEl) return;
    mainEl.innerHTML =
      '<header class="page-header"><h2>Page error</h2></header>' +
      '<p class="empty-state">' + (err && err.message ? err.message : String(err)) + '</p>';
  }

  function updateSidebarActive(hashPath) {
    const items = document.querySelectorAll('.sidebar [data-route]');
    items.forEach((el) => {
      const r = el.dataset.route;
      const active = (r === hashPath) || hashPath.startsWith(r + '/');
      el.classList.toggle('active', active);
    });
  }

  function navigate(hashPath) {
    window.location.hash = '#' + hashPath;
    // hashchange will fire dispatch.
  }

  function start() {
    window.addEventListener('hashchange', dispatch);
    // Defer initial dispatch to next tick so all page modules have
    // finished registering (script load order is deterministic but
    // explicit deferral matches Section 12.3.3 wiring discipline).
    setTimeout(dispatch, 0);
  }

  global.AuroraRouter = {
    register: register,
    setMain: setMain,
    navigate: navigate,
    dispatch: dispatch,
    resolve: resolve,
    start: start,
  };
})(window);
