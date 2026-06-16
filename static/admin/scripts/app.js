// Aurora Locus admin — app entry point.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §12.3.1 + §12.3.3. Bootstraps the
// router, mounts the sidebar, sets up theme/i18n, registers global
// keyboard handlers (command palette).
//
// This file replaces the old monolithic script.js bootstrap logic.
// All per-page logic lives in scripts/pages/<Name>.js.

(function (global) {
  'use strict';

  const POLL_INTERVAL_MS = 30_000;

  function ready(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }

  // The admin token lives in localStorage under 'aurora-admin-token'
  // (renamed from the legacy 'adminToken' per §8.1.1; session.js runs the
  // one-time migration at module load).

  async function bootstrap() {
    // Auth check first — bare fetch since AuroraSession is a thin
    // wrapper but the redirect-on-no-token logic lives here.
    if (!global.AuroraSession || !global.AuroraSession.token()) {
      window.location.href = '/admin/login.html';
      return;
    }

    // Load i18n strings before rendering anything.
    if (global.AuroraI18n) {
      const lang = global.AuroraSettings ? global.AuroraSettings.language() : 'en';
      try { await global.AuroraI18n.load(lang); } catch (e) { /* fall back to keys */ }
    }

    // Verify session.
    try {
      const sess = await global.AuroraEndpoints.atproto.getSession();
      global.AuroraSession.setUser(sess);
      const nameEl = document.getElementById('admin-name');
      if (nameEl) nameEl.textContent = sess.handle || 'Admin';
      const roleEl = document.getElementById('admin-role');
      if (roleEl) roleEl.textContent = sess.role || 'Operator';
    } catch (e) {
      global.AuroraSession.logout();
      return;
    }

    // Load the deployment moderation-mode before the first sidebar paint —
    // sidebar domain visibility (§5.7.4) depends on it. If the fetch fails
    // the cached default ('full') stands.
    await loadModerationMode();

    renderSidebar();

    // §5.8.4 — rebuild the sidebar when moderation-mode changes (e.g. an
    // operator switches it on Configuration → UI & modes, which calls
    // setModerationModeCache → AuroraSettings.notify). Only modMode
    // transitions matter here; theme/language changes are ignored.
    if (global.AuroraSettings) {
      let lastMode = global.AuroraSettings.getModerationMode();
      global.AuroraSettings.subscribe((s) => {
        if (s && s.modMode !== lastMode) {
          lastMode = s.modMode;
          renderSidebar();
        }
      });
    }

    // Wire router to the main mount point.
    const main = document.getElementById('content');
    if (global.AuroraRouter && main) {
      global.AuroraRouter.setMain(main);
      global.AuroraRouter.start();
    }

    // Command palette global keybinding.
    if (global.AuroraCommandPalette) global.AuroraCommandPalette.start();

    // Bell badge polling — refresh queue stats every 30s while tab focused.
    startQueueBadgePolling();
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) stopQueueBadgePolling();
      else startQueueBadgePolling();
    });

    // Capability cache warm — ensures hasCapability checks work post-mount.
    if (global.AuroraCapabilities) {
      global.AuroraCapabilities.getCapabilities().catch(() => {});
    }

    // Capability-cache refresh aligned with §6.17 60-min TTL handled
    // inside AuroraCapabilities; no app-level loop needed.
  }

  function mountSidebar() {
    const aside = document.getElementById('sidebar');
    if (!aside) return;
    const routes = global.AuroraRoutes;
    const sidebar = routes ? routes.sidebar : [];
    const session = global.AuroraSession;
    const role = session ? session.role() : 'moderator';
    const mode = global.AuroraSettings ? global.AuroraSettings.getModerationMode() : 'full';

    function domainVisible(domain) {
      return (routes && routes.domainVisible) ? routes.domainVisible(domain, role, mode) : true;
    }
    function itemAllowed(item) {
      return !(item.requires && session && !session.hasRole(item.requires));
    }

    let html = '';
    // §11.11.3 / §4.6 — the wordmark carries .heading-aurora so a theme can
    // paint it as a gradient logo (aurora-stack-classic does); the sober themes
    // leave it plain. Dormant until a theme styling it is served (B-themes-page).
    html += '<div class="logo">' +
            '  <h1 class="heading-aurora">Aurora Locus</h1>' +
            '  <p class="subtitle">Admin Panel</p>' +
            '</div>';
    html += '<nav class="nav-menu" aria-label="Primary navigation">';
    for (const node of sidebar) {
      if (node.heading) {
        // §5.7.4 — skip a domain the operator can't see in this role/mode.
        const domain = node.heading.toLowerCase();
        if (!domainVisible(domain)) continue;
        // Per-item role gate realises the "limited" cells of the matrix.
        const visibleItems = (node.items || []).filter(itemAllowed);
        // §5.8.3 — render the group label only when it has ≥1 visible item.
        if (visibleItems.length === 0) continue;
        html += '<div class="nav-section">';
        html += navSectionLabel(node);
        for (const item of visibleItems) html += navItem(item);
        html += '</div>';
      } else if (node.route) {
        const domain = (routes && routes.domainForPattern) ? routes.domainForPattern(node.route) : 'dashboard';
        if (!domainVisible(domain) || !itemAllowed(node)) continue;
        html += navItem(node);
      }
    }
    html += '</nav>';
    html += '<div class="sidebar-footer" id="sidebar-footer"></div>';
    aside.innerHTML = html;
  }

  // A group label. When the node carries a `route` the label is a link
  // (§5.8.2: clicking the Moderation label or its bell badge goes to the
  // Queue); the optional badge renders inside it, hidden until a non-zero
  // count arrives via refreshBadge().
  function navSectionLabel(node) {
    const badge = node.badgeId
      ? '<span class="badge" id="' + node.badgeId + '" style="display:none">0</span>'
      : '';
    const label = escHtml(node.heading);
    if (node.route) {
      return '<a class="nav-section-label nav-section-link" href="#' + node.route + '" data-route="' + node.route + '">' +
             label + badge + '</a>';
    }
    return '<span class="nav-section-label">' + label + badge + '</span>';
  }

  function navItem(item) {
    const icon = global.AuroraIcons ? global.AuroraIcons.render(item.icon || 'circle', 20) : '';
    const badge = item.badgeId
      ? '<span class="badge" id="' + item.badgeId + '">0</span>'
      : '';
    return '<a class="nav-item" href="#' + item.route + '" data-route="' + item.route + '">' +
           '<span class="icon" aria-hidden="true">' + icon + '</span>' +
           escHtml(item.label) + badge +
           '</a>';
  }

  function mountSidebarFooter() {
    const footer = document.getElementById('sidebar-footer');
    if (!footer) return;
    const session = global.AuroraSession;
    const handle = (session && session.user() && session.user().handle) || 'Admin';
    const role = (session && session.role()) || 'Operator';
    footer.innerHTML =
      '<div class="admin-info">' +
      '  <p class="admin-name" id="admin-name">' + escHtml(handle) + '</p>' +
      '  <p class="admin-role" id="admin-role">' + escHtml(role) + '</p>' +
      '</div>' +
      '<button class="btn-logout" id="sidebar-theme-toggle" type="button"></button>' +
      '<button class="btn-logout" id="logout-btn" type="button">Log out</button>';
    if (global.AuroraThemeToggle) {
      global.AuroraThemeToggle.mountCompact(document.getElementById('sidebar-theme-toggle'));
    }
    document.getElementById('logout-btn').addEventListener('click', () => session.logout());
  }

  function escHtml(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  // Fetch the deployment moderation-mode (and its redirect URL) into the
  // AuroraSettings cache. Mirrors ConfigUiModes.loadModerationMode; runs
  // once at boot and is harmless to call again.
  async function loadModerationMode() {
    if (!global.AuroraEndpoints || !global.AuroraSettings) return;
    try {
      const data = await global.AuroraEndpoints.admin.getRuntimeSetting('moderation-mode');
      const mode = (data && typeof data.value === 'string') ? data.value : 'full';
      let redirect;
      try {
        const r = await global.AuroraEndpoints.admin.getRuntimeSetting('moderation-mode-redirect-url');
        redirect = (r && typeof r.value === 'string') ? r.value : '';
      } catch (e) { /* redirect is optional */ }
      global.AuroraSettings.setModerationModeCache(mode, redirect);
    } catch (e) { /* cached default ('full') stands */ }
  }

  // Rebuild the whole sidebar (nav + footer) and re-apply the active
  // highlight and bell badge. Called at boot and on every mode change.
  function renderSidebar() {
    mountSidebar();
    mountSidebarFooter();
    refreshBadge();
    markActive((window.location.hash || '').replace(/^#\/?/, ''));
  }

  // Mark the nav entry (item or group label) matching the current hash.
  // Mirrors the router's own updateSidebarActive so a mode-driven
  // re-render without navigation doesn't drop the highlight.
  function markActive(hashPath) {
    document.querySelectorAll('.sidebar [data-route]').forEach((el) => {
      const r = el.dataset.route;
      const active = (r === hashPath) || (r && hashPath.indexOf(r + '/') === 0);
      el.classList.toggle('active', active);
    });
  }

  // ----- Bell badge polling -----
  let pollHandle = null;

  async function refreshBadge() {
    if (!global.AuroraEndpoints) return;
    // The badge exists only when the Moderation domain is visible
    // (Moderator+ in full mode), so its absence is the mode/role gate.
    const badge = document.getElementById('mod-attention-count');
    if (!badge) return;
    try {
      const stats = await global.AuroraEndpoints.admin.getQueueStats();
      if (!stats) return;
      // §5.8.2 — combined count of open reports + pending appeals.
      const count = (stats.openReports || 0) + (stats.pendingAppeals || stats.openAppeals || 0);
      badge.textContent = String(count);
      badge.classList.toggle('badge-attention', count > 0);
      badge.style.display = count > 0 ? '' : 'none';
    } catch (e) {
      // network/auth — leave badge unchanged
    }
  }

  function startQueueBadgePolling() {
    if (pollHandle) return;
    refreshBadge();
    pollHandle = setInterval(refreshBadge, POLL_INTERVAL_MS);
  }

  function stopQueueBadgePolling() {
    if (pollHandle) { clearInterval(pollHandle); pollHandle = null; }
  }

  ready(bootstrap);
})(window);
