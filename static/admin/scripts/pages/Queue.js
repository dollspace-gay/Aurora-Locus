// Moderation Queue page (route: #mod/queue).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.1.

(function (global) {
  'use strict';

  let bulkSelected = new Set();
  let lastFilters = {};

  // §5.3.1 / #209: the queue header status filter, restored with backend
  // wiring. Only the Status facet is surfaced: the queue is reports-only, so
  // the design's Type facet would need the (unbuilt) appeals-merge, and
  // Subject/Date have no queue-path backend — adding them would re-create the
  // decorative no-op filter that #209 was filed to remove. Scalar `status`
  // round-trips through the URL hash via the shared AuroraListPage shape
  // (components/ListPage.js, #257), so it survives navigation and reload.
  const SCALAR_KEYS = ['status'];
  const BOOL_KEYS = [];

  function applyFilters(vals) {
    global.AuroraListPage.applyFilters(SCALAR_KEYS, BOOL_KEYS, vals, null, function (v) {
      lastFilters = v;
      refresh();
    });
  }

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Queue</h2><p class="page-subtitle">Items needing attention</p></div>' +
      '</header>' +
      '<div id="queue-filter"></div>' +
      '<p class="filter-url-hint">' + (global.t ? global.t('common.filters_in_url') : '') + '</p>' +
      '<div id="queue-bulk-bar"></div>' +
      '<div class="moderation-queue" id="queue-items"></div>';
    bulkSelected = new Set();
    // Default to open-only, matching the backend's no-param behavior and the
    // §5.3.1 "items needing attention" framing.
    lastFilters = global.AuroraListPage.readFilters(SCALAR_KEYS, BOOL_KEYS, { status: 'open' });
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('queue-filter'),
        filters: [
          { type: 'select', id: 'status', label: 'Status', options: [
            { value: 'open', label: 'Open' },
            { value: 'acknowledged', label: 'Acknowledged' },
            { value: 'escalated', label: 'Escalated' },
            { value: 'resolved', label: 'Resolved' },
            { value: 'all', label: 'All' },
          ] },
        ],
        initial: lastFilters,
        onApply: applyFilters,
      });
    }
    await refresh();
    return { unmount: () => { bulkSelected = new Set(); } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('queue-items');
    if (!c || !ep) return;
    const params = { limit: 50 };
    // Omit status only when unset; the default 'open' and explicit 'all' both
    // round-trip ('all' clears the filter server-side).
    if (lastFilters.status) params.status = lastFilters.status;
    try {
      const data = await ep.atproto.getModerationQueue(params);
      // Canonical key is `queue` (get_moderation_queue returns {queue, count});
      // tolerate `items` defensively. Reading only `items` left the list
      // permanently empty.
      const items = (data && (data.queue || data.items)) || [];
      if (items.length === 0) {
        // §5.3.1: distinguish a filtered miss from a genuinely empty queue.
        const filterActive = lastFilters.status && lastFilters.status !== 'open';
        if (filterActive) {
          c.innerHTML = (global.AuroraEmptyState
            ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No matches.', secondary: 'Try widening your filters.' })
            : '<p class="empty-state">No matches.</p>') +
            '<p class="empty-state-action"><a href="#" id="queue-clear-filters">Clear all filters</a></p>';
          const clear = document.getElementById('queue-clear-filters');
          if (clear) clear.addEventListener('click', function (e) {
            e.preventDefault();
            applyFilters({ status: 'open' });
          });
        } else {
          c.innerHTML = global.AuroraEmptyState
            ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'Nothing in the queue.', secondary: 'Things will appear here as reports and appeals come in.' })
            : '<p class="empty-state">Nothing in the queue.</p>';
        }
        bulkSelected = new Set();
        renderBulkBar();
        return;
      }
      const visible = new Set(items.map((i) => i.subjectDid || (i.subject && i.subject.did)).filter(Boolean));
      bulkSelected = new Set([...bulkSelected].filter((d) => visible.has(d)));
      const isSuper = global.AuroraSession && global.AuroraSession.hasRole('superadmin');
      c.innerHTML = items.map((item) => {
        const subjDid = item.subjectDid || (item.subject && item.subject.did) || '';
        const checked = bulkSelected.has(subjDid) ? 'checked' : '';
        const cbDisabled = subjDid ? '' : 'disabled aria-label="No subject DID for this item"';
        // Real per-item status (queue items are reports; status is "open"
        // for everything the queue returns) — not a hardcoded 'pending'.
        const itemStatus = item.status || 'open';
        // §5.5.4 Phase D — escalated indicator + orphan affordance (§5.5 MD-43).
        const isEscalated = itemStatus === 'escalated';
        const isOrphan = isEscalated && !item.assignedOperatorDid;
        return '<div class="mod-item">' +
               '  <div class="mod-header">' +
               '    <input type="checkbox" class="bulk-select-mod" data-did="' + esc(subjDid) +
               '"' + (checked ? ' checked' : '') + ' ' + cbDisabled +
               ' aria-label="Select queue item ' + esc(item.id) + '">' +
               '    <div>' +
               '      <strong>' + esc(item.reasonType || 'Unknown') + '</strong>' +
               '      <p>By: ' + (subjDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(subjDid, item.reportedBy) : esc(subjDid)) : esc(item.reportedBy || '')) + '</p>' +
               '    </div>' +
               '    ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(itemStatus) : '<span class="status-badge status-' + esc(itemStatus) + '">' + esc(itemStatus) + '</span>') +
               '  </div>' +
               '  <div class="mod-content">' + esc(item.content || 'No content preview available') + '</div>' +
               (isOrphan ? '  <div class="mod-orphan-marker" style="color:#c60; font-weight:600;">Escalated, awaiting assignment</div>' : '') +
               '  <div class="mod-actions">' +
               (subjDid ? '<a class="btn-sm btn-secondary" href="#ops/accounts/' + encodeURIComponent(subjDid) + '">Open account</a>' : '') +
               (isEscalated && isSuper ? ' <button type="button" class="btn-sm mod-deescalate" data-id="' + esc(item.id) + '">De-escalate</button>' : '') +
               '  </div>' +
               '</div>';
      }).join('');
      c.querySelectorAll('.mod-deescalate').forEach((b) => {
        b.addEventListener('click', () => deescalate(b.getAttribute('data-id')));
      });
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
      global.AuroraErrorBoundary.mount(c, {
        message: 'Could not load queue: ' + ((e && e.message) || ''),
        onRetry: refresh,
      });
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
  // §5.5.4 Phase D — de-escalate a queue item (SuperAdmin). Prompts for a
  // rationale, calls clearEscalation, refreshes.
  async function deescalate(itemId) {
    const confirmResult = await global.AuroraModal.destructiveConfirm({
      heading: 'De-escalate item',
      body: 'Clear the escalation on this item? It returns to the queue (acknowledged) and re-routes per the current assignment mode.',
      confirmLabel: 'De-escalate',
      promptLabel: 'Rationale (required)',
    });
    if (!confirmResult.confirmed) return;
    const rationale = (confirmResult.promptValue || '').trim();
    if (!rationale) { global.AuroraToast.warning('Rationale is required.'); return; }
    try {
      await global.AuroraEndpoints.admin.clearEscalation({ itemId: itemId, rationale: rationale });
      global.AuroraToast.success('Item de-escalated.');
      refresh();
    } catch (e) {
      global.AuroraToast.danger('De-escalate failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('modQueue', { mount: mount });
})(window);
