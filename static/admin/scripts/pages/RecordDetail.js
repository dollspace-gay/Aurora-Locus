// Record detail page (route: #ops/records/:rest).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.2.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const uri = decodeURIComponent(params && params.rest || '');
    const m = /^at:\/\/([^/]+)\/([^/]+)\/([^/]+)$/.exec(uri);
    if (!m) {
      container.innerHTML =
        '<header class="page-header"><div><h2>Invalid record URI</h2></div></header>' +
        '<p class="empty-state">Expected at://did/collection/rkey form.</p>';
      return {};
    }
    const ownerDid = m[1];
    const collection = m[2];
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#ops/accounts">Operations</a> <span class="breadcrumb-sep">›</span> Records <span class="breadcrumb-sep">›</span> <code>' + esc(uri) + '</code></nav>' +
      '<header class="detail-header">' +
      '  <h2>Record</h2>' +
      '  <p class="meta"><code>' + esc(uri) + '</code></p>' +
      '</header>' +
      '<div class="detail-layout">' +
      '  <div class="detail-primary" id="rd-primary"><p class="empty-state">Loading…</p></div>' +
      '  <aside class="detail-rail" aria-label="Context">' +
      '    <div class="rail-card"><h4>Owning account</h4><div id="rd-owner">' +
      (global.AuroraEntityRef ? global.AuroraEntityRef.account(ownerDid) : '<code>' + esc(ownerDid) + '</code>') +
      '</div></div>' +
      '    <div class="rail-card" id="rd-context"><h4>Subject context</h4><p class="empty-state">Loading…</p></div>' +
      '    <div class="rail-card" id="rd-history"><h4>Subject history</h4><p class="empty-state">Loading…</p></div>' +
      '  </aside>' +
      '</div>';

    try {
      const data = await global.AuroraEndpoints.atproto.getRecord({ repo: ownerDid, collection: collection, rkey: m[3] });
      renderPrimary(data, uri, ownerDid);
    } catch (e) {
      document.getElementById('rd-primary').innerHTML =
        '<p class="empty-state">Could not load record: ' + esc(e && e.message) + '</p>';
    }
    loadContext(ownerDid, uri);
    return {};
  }

  function renderPrimary(data, uri, ownerDid) {
    const primary = document.getElementById('rd-primary');
    primary.innerHTML =
      '<div class="settings-card">' +
      '  <h3>Record content</h3>' +
      '  <pre style="white-space: pre-wrap; padding: 1rem; background: var(--background); border-radius: 6px; max-height: 320px; overflow-y: auto;">' +
      esc(JSON.stringify(data && data.value, null, 2)) + '</pre>' +
      '</div>' +
      '<div class="settings-card" style="margin-top: 1rem;">' +
      '  <h3>Moderation actions</h3>' +
      '  <div id="rd-action-panel"></div>' +
      '</div>';
    if (typeof ActionPanel === 'function') {
      const session = global.AuroraSession;
      const panel = new ActionPanel({
        subject: { '$type': 'com.atproto.repo.strongRef', uri: uri, cid: data && data.cid },
        availableActions: ['TakedownRecord', 'ApplyLabel', 'RemoveLabel'],
        defaultAction: 'TakedownRecord',
        requiresRationale: true,
        highImpactActions: ['TakedownRecord'],
        userRole: session ? session.role() : 'moderator',
      });
      panel.mount(document.getElementById('rd-action-panel'));
    }
  }

  async function loadContext(ownerDid, uri) {
    try {
      const ctx = await global.AuroraEndpoints.moderator.getSubjectContext({ subjectUri: uri });
      const card = document.getElementById('rd-context');
      const reports = (ctx && ctx.recentReports) || [];
      const actions = (ctx && ctx.recentActions) || [];
      card.innerHTML = '<h4>Subject context</h4>' +
        '<h5 style="margin: 0.5rem 0 0.25rem 0; font-size: 0.75rem; color: var(--text-tertiary);">Recent reports</h5>' +
        (reports.length ? '<ul style="list-style:none; padding:0;">' + reports.slice(0, 5).map((r) =>
          '<li>#' + esc(r.id) + ' — ' + esc(r.reasonType || '') + '</li>').join('') + '</ul>'
        : '<p style="color: var(--text-tertiary); font-size: 0.8125rem;">None</p>') +
        '<h5 style="margin: 0.5rem 0 0.25rem 0; font-size: 0.75rem; color: var(--text-tertiary);">Recent actions</h5>' +
        (actions.length ? '<ul style="list-style:none; padding:0;">' + actions.slice(0, 5).map((a) =>
          '<li>' + (global.AuroraEntityRef ? global.AuroraEntityRef.event(a.id) : '#' + esc(a.id)) + ' — ' + esc(a.eventType || a.action || '') + '</li>').join('') + '</ul>'
        : '<p style="color: var(--text-tertiary); font-size: 0.8125rem;">None</p>');
    } catch (e) {
      document.getElementById('rd-context').innerHTML = '<h4>Subject context</h4><p class="empty-state">Could not load.</p>';
    }
    try {
      const hist = await global.AuroraEndpoints.moderator.getSubjectHistory({ subjectUri: uri, limit: 10 });
      const items = (hist && hist.items) || [];
      const card = document.getElementById('rd-history');
      if (items.length === 0) {
        card.innerHTML = '<h4>Subject history</h4><p class="empty-state">No prior actions on this record.</p>';
      } else {
        const fmt = global.AuroraFormat;
        card.innerHTML = '<h4>Subject history</h4><ul style="list-style:none; padding:0;">' + items.map((it) =>
          '<li style="padding: 0.25rem 0;">' + esc(it.eventType || it.action || '') + ' — ' +
          (fmt ? esc(fmt.relativeTime(it.createdAt || it.timestamp)) : esc(it.createdAt || '')) +
          '</li>').join('') + '</ul>';
      }
    } catch (e) { /* ignore */ }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsRecordDetail', { mount: mount });
})(window);
