// Audit page (route: #mod/audit).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.8.

(function (global) {
  'use strict';

  let cursorStack = [];
  let nextCursor = null;
  let lastFilters = {};
  let subscription = null;

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Audit Trail</h2><p class="page-subtitle">Hash-chained audit log via tools.aurora.admin.getAuditTrail</p></div>' +
      '  <div id="audit-rt-indicator" class="rt-indicator-slot"></div>' +
      '</header>' +
      '<div id="audit-filter"></div>' +
      '<div id="audit-table-container"></div>' +
      '<div id="audit-pagination"></div>';
    cursorStack = [];
    nextCursor = null;
    lastFilters = {};
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('audit-filter'),
        filters: [
          { type: 'text', id: 'actor', placeholder: 'Filter by actor DID' },
          { type: 'text', id: 'subject', placeholder: 'Filter by subject DID' },
          { type: 'text', id: 'subjectCid', placeholder: 'Filter by subject CID' },
          { type: 'text', id: 'action', placeholder: 'Filter by action' },
          { type: 'checkbox', id: 'verifiedOnly', label: 'Verified only' },
          { type: 'dateRange', id: 'when', label: 'Date range' },
        ],
        onApply: (vals) => { lastFilters = vals; cursorStack = []; nextCursor = null; refresh(null); },
      });
    }
    await refresh(null);
    startSubscription();
    return {
      unmount: () => {
        if (subscription) { try { subscription.unsubscribe(); } catch (e) {} subscription = null; }
      },
    };
  }

  function startSubscription() {
    if (subscription || !global.AuroraSubscription) return;
    const indicator = document.getElementById('audit-rt-indicator');
    subscription = global.AuroraSubscription.subscribe('subscribe-mod-events', {}, {
      onEvent: () => { if (cursorStack.length === 0) refresh(null); },
      onError: (e) => console.warn('audit subscription error:', e),
    });
    if (indicator) global.AuroraSubscription.attachIndicator(indicator, subscription);
  }

  async function refresh(cursor) {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('audit-table-container');
    if (!c || !ep) return;
    const params = { limit: 25 };
    if (lastFilters.actor) params.actorDid = lastFilters.actor;
    if (lastFilters.subject) params.subjectDid = lastFilters.subject;
    if (lastFilters.subjectCid) params.subjectCid = lastFilters.subjectCid;
    if (lastFilters.action) params.action = lastFilters.action;
    if (cursor) params.cursor = cursor;
    if (lastFilters.when && lastFilters.when.start) params.since = lastFilters.when.start.toISOString();
    if (lastFilters.when && lastFilters.when.end) params.until = lastFilters.when.end.toISOString();
    c.innerHTML = '<p class="empty-state">Loading…</p>';
    try {
      const data = await ep.admin.getAuditTrail(params);
      let items = (data && data.items) || [];
      nextCursor = data && data.cursor;
      if (lastFilters.verifiedOnly) items = items.filter((e) => e.verified);
      if (items.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No audit entries match these filters.' })
          : '<p class="empty-state">No entries.</p>';
        renderPagination();
        return;
      }
      const fmt = global.AuroraFormat;
      let html = '<table class="data-table"><thead><tr>' +
                 '<th>Seq</th><th>When</th><th>Actor</th><th>Action</th><th>Subject</th><th>Verified</th><th></th>' +
                 '</tr></thead><tbody>';
      window._auditCache = window._auditCache || {};
      for (const e of items) {
        window._auditCache[e.id] = e;
        const subj = e.subjectRef ? (e.subjectRef.did || e.subjectRef.uri || e.subjectRef.cid || '—') : '—';
        const verifiedBadge = e.verified
          ? '<span class="status-badge status-verified" title="Hash matches stored chain hash">✓ verified</span>'
          : '<span class="status-badge status-suspended" title="Hash does not match — possibly tampered or pre-chain">✗ unverified</span>';
        html += '<tr>' +
                '<td>' + esc(e.sequence) + '</td>' +
                '<td>' + esc(fmt ? fmt.date(e.timestamp, 'short') : e.timestamp) + '</td>' +
                '<td>' + (e.actorDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(e.actorDid) : '<code>' + esc(e.actorDid) + '</code>') : '—') + '</td>' +
                '<td>' + esc(e.action) + '</td>' +
                '<td><code>' + esc(subj) + '</code></td>' +
                '<td>' + verifiedBadge + '</td>' +
                '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.audit(e.id) : '#' + esc(e.id)) + '</td>' +
                '</tr>';
      }
      html += '</tbody></table>';
      c.innerHTML = html;
      renderPagination();
    } catch (e) {
      c.innerHTML = '<p class="empty-state">Could not load audit: ' + esc(e && e.message) + '</p>';
    }
  }

  function renderPagination() {
    const c = document.getElementById('audit-pagination');
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
  if (global.AuroraRouter) global.AuroraRouter.register('modAudit', { mount: mount });
})(window);
