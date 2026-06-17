// Report detail page (route: #mod/reports/:id).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.3.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const id = params && params.id;
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#mod/reports">Moderation</a> <span class="breadcrumb-sep">›</span> <a href="#mod/reports">Reports</a> <span class="breadcrumb-sep">›</span> #' + esc(id) + '</nav>' +
      '<header class="page-header"><div><h2>Report #' + esc(id) + '</h2></div></header>' +
      '<div id="rd-body"><p class="empty-state">Loading…</p></div>';

    try {
      const data = await global.AuroraEndpoints.atproto.getReport({ id: id });
      renderBody(data);
    } catch (e) {
      document.getElementById('rd-body').innerHTML =
        '<p class="empty-state">Could not load report: ' + esc(e && e.message) + '</p>';
    }
    return {};
  }

  function renderBody(r) {
    const fmt = global.AuroraFormat;
    const subjDid = r.subjectDid || (r.subject && r.subject.did);
    const reporter = r.reportedBy;
    const body = document.getElementById('rd-body');
    body.innerHTML =
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Report metadata</h3>' +
      '    <p><strong>Reason:</strong> ' + esc(r.reasonType || 'Unknown') + '</p>' +
      '    <p><strong>Reported:</strong> ' + global.AuroraTimestamp.render({ value: r.reportedAt || r.createdAt, context: 'detail' }) + '</p>' +
      '    <p><strong>Reporter:</strong> ' + (reporter ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(reporter) : esc(reporter)) : '—') + '</p>' +
      '    <p><strong>Subject:</strong> ' + (subjDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(subjDid) : esc(subjDid)) : esc(r.subject || '')) + '</p>' +
      '    <p><strong>Status:</strong> ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(r.status || 'open', r.status || 'open') : '') + '</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Report reason</h3>' +
      '    <p>' + esc(r.reason || 'No reason provided') + '</p>' +
      '  </div>' +
      '</div>' +
      (subjDid ? '<div class="settings-card" style="margin-top: 1rem;">' +
                '<h3>Action against subject</h3>' +
                '<div id="rd-action-panel"></div></div>' : '');

    if (subjDid && typeof ActionPanel === 'function') {
      const session = global.AuroraSession;
      const panel = new ActionPanel({
        subject: { '$type': 'com.atproto.admin.defs#repoRef', did: subjDid },
        availableActions: ['TakedownAccount', 'SuspendAccount', 'RestoreAccount', 'ApplyLabel', 'RemoveLabel'],
        defaultAction: 'TakedownAccount',
        requiresRationale: true,
        highImpactActions: ['TakedownAccount'],
        userRole: session ? session.role() : 'moderator',
        onCancel: () => {},
      });
      panel.mount(document.getElementById('rd-action-panel'));
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modReportDetail', { mount: mount });
})(window);
