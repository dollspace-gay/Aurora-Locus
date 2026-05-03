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

  // localStorage key migration per §12.8: future v0.3 will move
  // 'adminToken' → 'aurora-admin-token'. v0.2 preserves the old key.

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

    mountSidebar();
    mountSidebarFooter();

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
    const sidebar = global.AuroraRoutes ? global.AuroraRoutes.sidebar : [];
    const session = global.AuroraSession;
    let html = '';
    html += '<div class="logo">' +
            '  <h1>Aurora Locus</h1>' +
            '  <p class="subtitle">Admin Panel</p>' +
            '</div>';
    html += '<nav class="nav-menu" aria-label="Primary navigation">';
    for (const node of sidebar) {
      if (node.heading) {
        // Skip whole sections the operator can't see at all.
        if (node.requires && session && !session.hasRole(node.requires)) continue;
        html += '<div class="nav-section">';
        html += '<span class="nav-section-label">' + escHtml(node.heading) + '</span>';
        for (const item of node.items || []) {
          if (item.requires && session && !session.hasRole(item.requires)) continue;
          html += navItem(item);
        }
        html += '</div>';
      } else if (node.route) {
        html += navItem(node);
      }
    }
    html += '</nav>';
    html += '<div class="sidebar-footer" id="sidebar-footer"></div>';
    aside.innerHTML = html;
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

  // ----- Bell badge polling -----
  let pollHandle = null;

  async function refreshBadge() {
    if (!global.AuroraEndpoints) return;
    try {
      const stats = await global.AuroraEndpoints.admin.getQueueStats();
      if (!stats) return;
      const badge = document.getElementById('mod-queue-count');
      if (badge) {
        badge.textContent = String(stats.queueAttentionTotal || 0);
        badge.classList.toggle('badge-attention', (stats.queueAttentionTotal || 0) > 0);
      }
      const reports = document.getElementById('reports-count');
      if (reports) reports.textContent = String(stats.openReports || 0);
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
