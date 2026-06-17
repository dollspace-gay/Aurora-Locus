// Event detail page (route: #mod/events/:id).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.7.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const id = params && params.id;
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#mod/events">Moderation</a> <span class="breadcrumb-sep">›</span> <a href="#mod/events">Events</a> <span class="breadcrumb-sep">›</span> #' + esc(id) + '</nav>' +
      '<header class="page-header"><div><h2>Event #' + esc(id) + '</h2></div></header>' +
      '<div id="ed-body"><p class="empty-state">Loading…</p></div>';
    try {
      const data = await global.AuroraEndpoints.moderator.getEvent(id);
      renderBody(data);
    } catch (e) {
      document.getElementById('ed-body').innerHTML =
        '<p class="empty-state">Could not load event: ' + esc(e && e.message) + '</p>';
    }
    return {};
  }

  function renderBody(e) {
    const fmt = global.AuroraFormat;
    const subj = e.subject ? (global.AuroraEntityRef ? global.AuroraEntityRef.fromSubject(e.subject) : esc(JSON.stringify(e.subject))) : '—';
    const body = document.getElementById('ed-body');
    body.innerHTML =
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Event metadata</h3>' +
      '    <p><strong>Type:</strong> ' + esc(e.eventType) + '</p>' +
      '    <p><strong>When:</strong> ' + global.AuroraTimestamp.render({ value: e.createdAt, context: 'detail' }) + '</p>' +
      '    <p><strong>Actor:</strong> ' + (e.actorDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(e.actorDid, e.actorHandle) : esc(e.actorDid)) : '—') + '</p>' +
      '    <p><strong>Subject:</strong> ' + subj + '</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Rationale</h3>' +
      '    <p>' + esc(e.rationale || e.reason || '—') + '</p>' +
      '  </div>' +
      '</div>' +
      '<div class="settings-card" style="margin-top: 1rem;">' +
      '  <h3>Raw event payload</h3>' +
      '  <pre style="white-space: pre-wrap; padding: 1rem; background: var(--color-surface-primary); border-radius: 6px; max-height: 400px; overflow-y: auto;">' +
      esc(JSON.stringify(e, null, 2)) + '</pre>' +
      '</div>';
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modEventDetail', { mount: mount });
})(window);
