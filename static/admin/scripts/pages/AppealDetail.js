// Appeal detail page (route: #mod/appeals/:id).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.5.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const id = params && params.id;
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#mod/appeals">Moderation</a> <span class="breadcrumb-sep">›</span> <a href="#mod/appeals">Appeals</a> <span class="breadcrumb-sep">›</span> #' + esc(id) + '</nav>' +
      '<header class="page-header"><div><h2>Appeal #' + esc(id) + '</h2></div></header>' +
      '<div id="apd-body">' + global.AuroraSkeleton.lines(4) + '</div>';
    await loadAppeal(id);
    return {};
  }

  async function loadAppeal(id) {
    const body = document.getElementById('apd-body');
    if (!body) return;
    body.innerHTML = global.AuroraSkeleton.lines(4);
    try {
      const data = await global.AuroraEndpoints.moderator.getAppeal(id);
      renderBody(data);
    } catch (e) {
      global.AuroraErrorBoundary.mount(body, {
        message: 'Could not load appeal: ' + ((e && e.message) || ''),
        onRetry: function () { loadAppeal(id); },
      });
    }
  }

  function renderBody(a) {
    const fmt = global.AuroraFormat;
    const subj = a.subject ? (global.AuroraEntityRef ? global.AuroraEntityRef.fromSubject(a.subject) : esc(JSON.stringify(a.subject))) : '—';
    const orig = a.originalActionSummary
      ? esc(a.originalActionSummary.kind + ' #' + a.originalActionSummary.id + ': ' + (a.originalActionSummary.summary || ''))
      : '—';
    const body = document.getElementById('apd-body');
    body.innerHTML =
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Appeal metadata</h3>' +
      '    <p><strong>Submitted:</strong> ' + global.AuroraTimestamp.render({ value: a.submittedAt, context: 'detail' }) + '</p>' +
      '    <p><strong>Status:</strong> ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(a.status, a.status) : esc(a.status)) + '</p>' +
      '    <p><strong>Appellant:</strong> ' + (a.submitterDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(a.submitterDid, a.submitterHandle) : esc(a.submitterDid)) : '—') + '</p>' +
      '    <p><strong>Subject:</strong> ' + subj + '</p>' +
      '    <p><strong>Original action:</strong> ' + orig + '</p>' +
      '    <p><strong>Reviewer:</strong> ' + (a.reviewerDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(a.reviewerDid) : esc(a.reviewerDid)) : '—') + '</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Reason</h3>' +
      '    <p>' + esc(a.reason || '') + '</p>' +
      (a.responseRationale ? '<h4 style="margin-top: 1rem;">Resolution rationale</h4><p>' + esc(a.responseRationale) + '</p>' : '') +
      '  </div>' +
      '</div>' +
      ((a.status === 'pending' || a.status === 'under_review') ? '<div class="settings-card" style="margin-top: 1rem;">' +
        '<h3>Resolve appeal</h3>' +
        '<div id="apd-resolve">' + resolveForm() + '</div></div>' : '');

    if (a.status === 'pending' || a.status === 'under_review') {
      wireResolveForm(a.id);
    }
  }

  function resolveForm() {
    return '<div class="form-group">' +
           '  <label>Decision</label>' +
           '  <select id="apd-decision">' +
           '    <option value="approved">Approved</option>' +
           '    <option value="denied">Denied</option>' +
           '    <option value="escalated">Escalated</option>' +
           '  </select>' +
           '</div>' +
           '<div class="form-group">' +
           '  <label>Rationale (required)</label>' +
           '  <textarea id="apd-rationale" rows="3" style="width:100%;"></textarea>' +
           '</div>' +
           '<button class="btn-primary" id="apd-submit">Resolve appeal</button>';
  }

  function wireResolveForm(id) {
    const btn = document.getElementById('apd-submit');
    if (!btn) return;
    btn.addEventListener('click', async () => {
      const status = document.getElementById('apd-decision').value;
      const rationale = document.getElementById('apd-rationale').value.trim();
      if (!rationale) { global.AuroraToast.warning('Rationale is required.'); return; }
      try {
        await global.AuroraEndpoints.moderator.resolveAppeal({
          id: id, status: status, rationale: rationale,
        });
        global.AuroraToast.success('Appeal resolved.');
        if (global.AuroraRouter) global.AuroraRouter.navigate('mod/appeals');
      } catch (e) {
        global.AuroraToast.danger('Resolve failed: ' + (e && e.message ? e.message : ''));
      }
    });
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modAppealDetail', { mount: mount });
})(window);
