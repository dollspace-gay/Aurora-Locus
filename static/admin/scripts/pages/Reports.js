// Reports list page (route: #mod/reports).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.2.

(function (global) {
  'use strict';

  let bulkSelected = new Set();
  let cursorStack = [];
  let nextCursor = null;
  let lastFilters = {};

  // url-state wiring (§5.7.5). Scalar filters round-trip through the hash
  // query; the dateRange `when` round-trips as since/until ISO scalars
  // (FilterStrip can't restore the date *chip* from initial, but the
  // filter still applies and is preserved across applies). applyFilters
  // writes the query, which remounts the page → readFilters re-seeds.
  const SCALAR_KEYS = ['status', 'reporter', 'subject'];
  const BOOL_KEYS = [];

  function readFilters(defaults) {
    const u = global.AuroraUrlState ? global.AuroraUrlState.read() : {};
    const f = Object.assign({}, defaults || {});
    for (const k of SCALAR_KEYS) { if (u[k]) f[k] = u[k]; }
    for (const k of BOOL_KEYS) { if (u[k]) f[k] = true; }
    if (u.since || u.until) {
      f.when = { start: u.since ? new Date(u.since) : null, end: u.until ? new Date(u.until) : null };
    }
    return f;
  }

  function applyFilters(vals) {
    const when = (vals && vals.when) || (lastFilters && lastFilters.when) || null;
    const u = {};
    for (const k of SCALAR_KEYS) { if (vals[k]) u[k] = vals[k]; }
    for (const k of BOOL_KEYS) { if (vals[k]) u[k] = '1'; }
    if (when && when.start) u.since = when.start.toISOString();
    if (when && when.end) u.until = when.end.toISOString();
    if (global.AuroraUrlState) global.AuroraUrlState.write(u);
    else { lastFilters = vals; cursorStack = []; nextCursor = null; refresh(null); }
  }

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Reports</h2><p class="page-subtitle">Content reports</p></div>' +
      '</header>' +
      '<div id="reports-filter"></div>' +
      '<p class="filter-url-hint">' + (global.t ? global.t('common.filters_in_url') : '') + '</p>' +
      '<div id="reports-bulk-bar"></div>' +
      '<div class="reports-list" id="reports-items"></div>' +
      '<div id="reports-pagination"></div>';
    bulkSelected = new Set();
    cursorStack = [];
    nextCursor = null;
    lastFilters = readFilters({ status: 'open' });
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('reports-filter'),
        filters: [
          { type: 'select', id: 'status', label: 'Status', options: [
            { value: 'open', label: 'Open' },
            { value: 'resolved', label: 'Resolved' },
            { value: '', label: 'All' },
          ] },
          { type: 'text', id: 'reporter', placeholder: 'Reporter DID' },
          { type: 'text', id: 'subject', placeholder: 'Subject DID' },
          { type: 'dateRange', id: 'when', label: 'Date range' },
        ],
        initial: lastFilters,
        onApply: applyFilters,
      });
    }
    await refresh(null);
    return { unmount: () => { bulkSelected = new Set(); } };
  }

  async function refresh(cursor) {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('reports-items');
    if (!c || !ep) return;
    const params = { limit: 50 };
    if (lastFilters.status) params.status = lastFilters.status;
    if (lastFilters.reporter) params.reporter = lastFilters.reporter;
    if (lastFilters.subject) params.subject = lastFilters.subject;
    if (cursor) params.cursor = cursor;
    if (lastFilters.when && lastFilters.when.start) params.since = lastFilters.when.start.toISOString();
    if (lastFilters.when && lastFilters.when.end) params.until = lastFilters.when.end.toISOString();

    try {
      const data = await ep.atproto.listReports(params);
      const reports = (data && data.reports) || [];
      nextCursor = data && data.cursor;
      if (reports.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No reports match.' })
          : '<p class="empty-state">No reports.</p>';
        renderPagination();
        renderBulkBar();
        return;
      }
      const visible = new Set(reports.map((r) => r.subjectDid || (r.subject && r.subject.did) ||
        (typeof r.subject === 'string' && r.subject.startsWith('did:') ? r.subject : null)).filter(Boolean));
      bulkSelected = new Set([...bulkSelected].filter((d) => visible.has(d)));

      c.innerHTML = reports.map((r) => {
        const subjDid = r.subjectDid || (r.subject && r.subject.did) ||
          (typeof r.subject === 'string' && r.subject.startsWith('did:') ? r.subject : '');
        const checked = bulkSelected.has(subjDid) ? 'checked' : '';
        const cbDisabled = subjDid ? '' : 'disabled aria-label="No DID-shaped subject"';
        return '<div class="report-item">' +
               '  <div class="report-header">' +
               '    <input type="checkbox" class="bulk-select-report" data-did="' + esc(subjDid) +
               '"' + (checked ? ' checked' : '') + ' ' + cbDisabled + '>' +
               '    <div>' +
               '      <strong>' + esc(r.reasonType || '') + '</strong>' +
               '      <p>Reporter: ' + (r.reportedBy ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(r.reportedBy) : esc(r.reportedBy)) : '—') + '</p>' +
               '      <p>Subject: ' + (subjDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(subjDid) : esc(subjDid)) : esc(r.subject || '')) + '</p>' +
               '    </div>' +
               '    ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(r.status || 'open', r.status || 'open') : '') +
               '  </div>' +
               '  <div class="report-content">' + esc(r.reason || 'No reason provided') + '</div>' +
               '  <div class="report-actions">' +
               '    <a class="btn-sm btn-primary" href="#mod/reports/' + encodeURIComponent(r.id) + '">View Details</a>' +
               '  </div>' +
               '</div>';
      }).join('');
      c.querySelectorAll('.bulk-select-report').forEach((cb) => {
        cb.addEventListener('change', (e) => {
          const did = e.target.dataset.did;
          if (!did) return;
          if (e.target.checked) bulkSelected.add(did);
          else bulkSelected.delete(did);
          renderBulkBar();
        });
      });
      renderBulkBar();
      renderPagination();
    } catch (e) {
      c.innerHTML = '<p class="empty-state">Could not load reports: ' + esc(e && e.message) + '</p>';
    }
  }

  function renderBulkBar() {
    const bar = document.getElementById('reports-bulk-bar');
    if (!bar) return;
    const n = bulkSelected.size;
    if (n === 0) { bar.innerHTML = ''; return; }
    bar.innerHTML =
      '<div class="bulk-action-bar" role="toolbar" aria-label="Bulk actions">' +
      '<span><strong>' + n + '</strong> subject' + (n === 1 ? '' : 's') + ' selected</span>' +
      '<button class="btn-sm btn-danger" id="rb-takedown">Bulk takedown</button>' +
      '<button class="btn-sm btn-secondary" id="rb-suspend">Bulk suspend</button>' +
      '<button class="btn-sm btn-secondary" id="rb-label">Bulk label</button>' +
      '<button class="btn-sm btn-secondary" id="rb-clear">Clear</button>' +
      '</div>';
    document.getElementById('rb-takedown').addEventListener('click', () => openBulk('BatchTakedownAccounts'));
    document.getElementById('rb-suspend').addEventListener('click', () => openBulk('BatchSuspendAccounts'));
    document.getElementById('rb-label').addEventListener('click', () => openBulk('BatchApplyLabel'));
    document.getElementById('rb-clear').addEventListener('click', () => {
      bulkSelected = new Set();
      document.querySelectorAll('.bulk-select-report').forEach((cb) => { cb.checked = false; });
      renderBulkBar();
    });
  }

  function openBulk(defaultAction) {
    const subjects = [...bulkSelected].map((did) => ({ '$type': 'com.atproto.admin.defs#repoRef', did: did }));
    if (subjects.length === 0) return;
    const div = document.createElement('div');
    const handle = global.AuroraModal.open({ title: 'Bulk action', body: div });
    const panel = new BulkActionPanel({
      subjects: subjects,
      availableActions: ['BatchTakedownAccounts', 'BatchSuspendAccounts', 'BatchApplyLabel'],
      onCancel: () => handle.close(),
    });
    panel.mount(div);
    if (panel.state) panel.state.action = defaultAction;
    panel.render();
  }

  function renderPagination() {
    const c = document.getElementById('reports-pagination');
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
        } else if (cursorStack.length === 1) {
          cursorStack = [];
          refresh(null);
        }
      },
      onNext: () => {
        if (nextCursor) { cursorStack.push(nextCursor); refresh(nextCursor); }
      },
    });
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modReports', { mount: mount });
})(window);
