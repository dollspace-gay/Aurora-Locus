// Appeals list page (route: #mod/appeals).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.4.

(function (global) {
  'use strict';

  let cursorStack = [];
  let nextCursor = null;
  let lastFilters = {};

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Appeals</h2><p class="page-subtitle">Cross-instance appeals via tools.aurora.moderator.listAppeals</p></div>' +
      '</header>' +
      '<div id="appeals-filter"></div>' +
      '<div id="appeals-table-container"></div>' +
      '<div id="appeals-pagination"></div>';
    cursorStack = [];
    nextCursor = null;
    lastFilters = {};
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('appeals-filter'),
        filters: [
          { type: 'select', id: 'status', label: 'Status', options: [
            { value: '', label: 'All statuses' },
            { value: 'pending', label: 'Pending' },
            { value: 'under_review', label: 'Under Review' },
            { value: 'approved', label: 'Approved' },
            { value: 'denied', label: 'Denied' },
            { value: 'escalated', label: 'Escalated' },
          ] },
          { type: 'text', id: 'appellant', placeholder: 'Filter by appellant DID' },
          { type: 'text', id: 'reviewer', placeholder: 'Filter by reviewer DID' },
          { type: 'dateRange', id: 'when', label: 'Date range' },
        ],
        onApply: (vals) => { lastFilters = vals; cursorStack = []; nextCursor = null; refresh(null); },
      });
    }
    await refresh(null);
    return {};
  }

  async function refresh(cursor) {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('appeals-table-container');
    if (!c || !ep) return;
    const params = { limit: 25 };
    if (lastFilters.status) params.status = lastFilters.status;
    if (lastFilters.appellant) params.appellant = lastFilters.appellant;
    if (lastFilters.reviewer) params.reviewer = lastFilters.reviewer;
    if (cursor) params.cursor = cursor;
    if (lastFilters.when && lastFilters.when.start) params.since = lastFilters.when.start.toISOString();
    if (lastFilters.when && lastFilters.when.end) params.until = lastFilters.when.end.toISOString();

    c.innerHTML = '<p class="empty-state">Loading…</p>';
    try {
      const data = await ep.moderator.listAppeals(params);
      const items = (data && data.items) || [];
      nextCursor = data && data.cursor;
      if (items.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No appeals match these filters.' })
          : '<p class="empty-state">No appeals.</p>';
        renderPagination();
        return;
      }
      const fmt = global.AuroraFormat;
      let html = '<table class="data-table"><thead><tr>' +
                 '<th>Submitted</th><th>Status</th><th>Appellant</th><th>Subject</th>' +
                 '<th>Original Action</th><th>Reason</th><th></th>' +
                 '</tr></thead><tbody>';
      for (const a of items) {
        const subjStr = a.subject ? subjectLink(a.subject) : '—';
        const orig = a.originalActionSummary
          ? (a.originalActionSummary.kind + ' #' + a.originalActionSummary.id + ': ' +
             (a.originalActionSummary.summary || ''))
          : '—';
        html += '<tr>' +
                '<td>' + esc(fmt ? fmt.date(a.submittedAt, 'short') : a.submittedAt || '') + '</td>' +
                '<td>' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(a.status, a.status) : esc(a.status)) + '</td>' +
                '<td>' + (a.submitterDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(a.submitterDid, a.submitterHandle) : esc(a.submitterDid)) : '—') + '</td>' +
                '<td>' + subjStr + '</td>' +
                '<td>' + esc(orig) + '</td>' +
                '<td>' + esc(a.reason || '') + '</td>' +
                '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.appeal(a.id) : '#' + esc(a.id)) + '</td>' +
                '</tr>';
      }
      html += '</tbody></table>';
      c.innerHTML = html;
      renderPagination();
    } catch (e) {
      c.innerHTML = '<p class="empty-state">Could not load appeals: ' + esc(e && e.message) + '</p>';
    }
  }

  function subjectLink(subject) {
    if (global.AuroraEntityRef) return global.AuroraEntityRef.fromSubject(subject);
    return esc(JSON.stringify(subject));
  }

  function renderPagination() {
    const c = document.getElementById('appeals-pagination');
    if (!c || !global.AuroraPagination) return;
    global.AuroraPagination.render({
      container: c,
      prevDisabled: cursorStack.length === 0,
      nextDisabled: !nextCursor,
      onPrev: () => {
        if (cursorStack.length > 1) {
          cursorStack.pop();
          const p = cursorStack[cursorStack.length - 1] || null;
          refresh(p);
        } else if (cursorStack.length === 1) { cursorStack = []; refresh(null); }
      },
      onNext: () => { if (nextCursor) { cursorStack.push(nextCursor); refresh(nextCursor); } },
    });
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modAppeals', { mount: mount });
})(window);
