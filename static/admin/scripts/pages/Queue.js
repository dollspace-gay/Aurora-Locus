// Moderation Queue page (route: #mod/queue).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.1.

(function (global) {
  'use strict';

  let bulkSelected = new Set();

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Queue</h2><p class="page-subtitle">Items needing attention</p></div>' +
      '  <div class="header-actions">' +
      '    <select class="filter-select" id="queue-filter">' +
      '      <option value="all">All Items</option>' +
      '      <option value="pending">Pending</option>' +
      '      <option value="reviewed">Reviewed</option>' +
      '    </select>' +
      '  </div>' +
      '</header>' +
      '<div id="queue-bulk-bar"></div>' +
      '<div class="moderation-queue" id="queue-items"></div>';
    bulkSelected = new Set();
    document.getElementById('queue-filter').addEventListener('change', refresh);
    await refresh();
    return { unmount: () => { bulkSelected = new Set(); } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('queue-items');
    if (!c || !ep) return;
    try {
      const data = await ep.atproto.getModerationQueue({ limit: 50 });
      const items = (data && data.items) || [];
      if (items.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'Nothing in the queue.', secondary: 'Things will appear here as reports and appeals come in.' })
          : '<p class="empty-state">Nothing in the queue.</p>';
        bulkSelected = new Set();
        renderBulkBar();
        return;
      }
      const visible = new Set(items.map((i) => i.subjectDid || (i.subject && i.subject.did)).filter(Boolean));
      bulkSelected = new Set([...bulkSelected].filter((d) => visible.has(d)));
      c.innerHTML = items.map((item) => {
        const subjDid = item.subjectDid || (item.subject && item.subject.did) || '';
        const checked = bulkSelected.has(subjDid) ? 'checked' : '';
        const cbDisabled = subjDid ? '' : 'disabled aria-label="No subject DID for this item"';
        return '<div class="mod-item">' +
               '  <div class="mod-header">' +
               '    <input type="checkbox" class="bulk-select-mod" data-did="' + esc(subjDid) +
               '"' + (checked ? ' checked' : '') + ' ' + cbDisabled +
               ' aria-label="Select queue item ' + esc(item.id) + '">' +
               '    <div>' +
               '      <strong>' + esc(item.reasonType || 'Unknown') + '</strong>' +
               '      <p>By: ' + (subjDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(subjDid, item.reportedBy) : esc(subjDid)) : esc(item.reportedBy || '')) + '</p>' +
               '    </div>' +
               '    ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render('pending', 'Pending') : '<span class="status-badge status-pending">Pending</span>') +
               '  </div>' +
               '  <div class="mod-content">' + esc(item.content || 'No content preview available') + '</div>' +
               '  <div class="mod-actions">' +
               (subjDid ? '<a class="btn-sm btn-secondary" href="#ops/accounts/' + encodeURIComponent(subjDid) + '">Open account</a>' : '') +
               '  </div>' +
               '</div>';
      }).join('');
      c.querySelectorAll('.bulk-select-mod').forEach((cb) => {
        cb.addEventListener('change', (e) => {
          const did = e.target.dataset.did;
          if (!did) return;
          if (e.target.checked) bulkSelected.add(did);
          else bulkSelected.delete(did);
          renderBulkBar();
        });
      });
      renderBulkBar();
    } catch (e) {
      c.innerHTML = '<p class="empty-state">Could not load queue: ' + esc(e && e.message) + '</p>';
    }
  }

  function renderBulkBar() {
    const bar = document.getElementById('queue-bulk-bar');
    if (!bar) return;
    const n = bulkSelected.size;
    if (n === 0) { bar.innerHTML = ''; return; }
    bar.innerHTML =
      '<div class="bulk-action-bar" role="toolbar" aria-label="Bulk actions">' +
      '<span><strong>' + n + '</strong> subject' + (n === 1 ? '' : 's') + ' selected</span>' +
      '<button class="btn-sm btn-danger" id="qb-takedown">Bulk takedown</button>' +
      '<button class="btn-sm btn-secondary" id="qb-suspend">Bulk suspend</button>' +
      '<button class="btn-sm btn-secondary" id="qb-clear">Clear</button>' +
      '</div>';
    document.getElementById('qb-takedown').addEventListener('click', () => openBulk('BatchTakedownAccounts'));
    document.getElementById('qb-suspend').addEventListener('click', () => openBulk('BatchSuspendAccounts'));
    document.getElementById('qb-clear').addEventListener('click', () => {
      bulkSelected = new Set();
      document.querySelectorAll('.bulk-select-mod').forEach((cb) => { cb.checked = false; });
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

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modQueue', { mount: mount });
})(window);
