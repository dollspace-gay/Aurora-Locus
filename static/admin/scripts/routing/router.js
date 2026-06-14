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
    // Strip any filter-state query string (§5.7.5) — a route pattern
    // never contains '?', so route matching, legacy redirects, and the
    // active-nav highlight all operate on the path alone. Pages read the
    // query via AuroraUrlState.read().
    const q = hash.indexOf('?');
    return q >= 0 ? hash.slice(0, q) : hash;
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
    // §5.7.4 mode gate — a route whose top-level domain is hidden by the
    // current moderation-mode is unreachable even by deep link (e.g. any
    // non-Configuration route in `disabled` mode, or Moderation in
    // `reduced`). This is the MODE dimension only; the route's own
    // `requires` above is the authoritative role gate, so Moderator-level
    // routes that sit under an Admin+ sidebar group stay reachable.
    if (global.AuroraRoutes && global.AuroraRoutes.domainModeAllowed) {
      const mode = global.AuroraSettings ? global.AuroraSettings.getModerationMode() : 'full';
      const domain = global.AuroraRoutes.domainForPattern(match.route.pattern);
      if (!global.AuroraRoutes.domainModeAllowed(domain, mode)) {
        mountModeUnavailable(match.route, mode);
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
    // Tear-down: drop any previous render. Use replaceChildren() rather
    // than `innerHTML = ''` so the whole module is consistent in not
    // assigning to innerHTML — see the renderMessage helper below for
    // the rationale (URL-derived content goes through textContent only).
    if (mainEl) mainEl.replaceChildren();
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

  // Render a banner page (heading + paragraph) into mainEl using DOM
  // construction primitives only. Every dynamic value reaches the page
  // through `textContent`, never through `innerHTML`, so URL-derived
  // and exception-derived inputs cannot inject markup. Three call
  // sites (mountNotFound / mountForbidden / mountError) all flow
  // through here; the "untrusted text + maybe an inline-styled span"
  // shape is uniform.
  //
  // The previous implementation interpolated raw strings into an
  // innerHTML template literal — a textbook XSS sink because the hash
  // and any error message might originate in attacker-controlled URLs.
  // A successful XSS in the admin UI hands the attacker the operator's
  // localStorage 'aurora-admin-token' and from there every admin XRPC the
  // operator has scope for. Treat all inputs here as hostile.
  function renderMessage(headingText, bodyParts) {
    if (!mainEl) return;
    mainEl.replaceChildren();
    const header = document.createElement('header');
    header.className = 'page-header';
    const h2 = document.createElement('h2');
    h2.textContent = headingText;
    header.appendChild(h2);
    const p = document.createElement('p');
    p.className = 'empty-state';
    for (const part of bodyParts) {
      if (part == null) continue;
      if (typeof part === 'string') {
        p.appendChild(document.createTextNode(part));
      } else if (part.tag && typeof part.text === 'string') {
        // Wrapped fragment: e.g. {tag:'code', text:path} renders as
        // <code>{escape(path)}</code>. The tag is from a static
        // call-site literal here; only `text` is dynamic.
        const span = document.createElement(part.tag);
        span.textContent = part.text;
        p.appendChild(span);
      }
    }
    mainEl.appendChild(header);
    mainEl.appendChild(p);
  }

  function mountNotFound(path, msg) {
    if (msg) {
      renderMessage('Page not found', [msg]);
    } else {
      renderMessage('Page not found', [
        'No route matches: ',
        { tag: 'code', text: '#' + (path || '') },
      ]);
    }
  }

  function mountForbidden(route) {
    // route.requires is a static string from the routes table, not
    // URL-derived; treating it as text is still the right move
    // because (a) the same render path covers both trusted and
    // untrusted inputs, removing one branch per call site, and (b)
    // future contributors don't have to remember which side of the
    // trusted/untrusted line each variable is on.
    renderMessage('Access denied', [
      'This page requires the ',
      { tag: 'strong', text: route.requires || 'higher' },
      ' role.',
    ]);
  }

  function mountError(err) {
    const message = err && err.message ? String(err.message) : String(err);
    renderMessage('Page error', [message]);
  }

  // §5.7.4 / §5.5 — a route whose domain is hidden in the current
  // moderation mode. In disabled mode the deployment may configure a
  // landing redirect; we honour it but only as an INTERNAL hash target
  // (stripping any leading '#') so a runtime-setting string can't become
  // an open redirect to an arbitrary origin. Otherwise we explain and
  // point at Configuration, which is always reachable.
  function mountModeUnavailable(route, mode) {
    if (mode === 'disabled' && global.AuroraSettings && global.AuroraSettings.getModerationModeRedirect) {
      const redirect = String(global.AuroraSettings.getModerationModeRedirect() || '');
      const target = redirect.replace(/^#\/?/, '');
      if (target && target !== (route.pattern || '')) {
        window.location.replace('#' + target);
        return;
      }
    }
    renderMessage('Unavailable in this mode', [
      'This area is not available while moderation mode is ',
      { tag: 'strong', text: mode || 'reduced' },
      '. Open ',
      { tag: 'code', text: '#configuration/general' },
      ' to change the mode, or continue in an available area.',
    ]);
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
