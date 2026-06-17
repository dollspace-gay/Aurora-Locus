// Accounts list page (route: #ops/accounts).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.1. Search by handle/DID/email,
// filter chips, paginated table. Click row → Account detail.
// Bulk multi-select toggle exposes batch operations.

(function (global) {
  'use strict';

  let bulkSelected = new Set();

  async function mount({ container }) {
    const ep = global.AuroraEndpoints;
    container.innerHTML = renderShell();
    bulkSelected = new Set();
    const search = container.querySelector('#account-search');
    // url-state (§5.7.5): seed the search box from the hash query so a
    // pasted/bookmarked #ops/accounts?q=… restores the filtered view.
    const urlF = global.AuroraUrlState ? global.AuroraUrlState.read() : {};
    if (search && urlF.q) search.value = urlF.q;
    if (search) search.addEventListener('input', debounce(refresh, 400));
    await refresh();

    return { unmount: () => { bulkSelected = new Set(); } };
  }

  function renderShell() {
    return '<header class="page-header">' +
           '  <div>' +
           '    <h2>Accounts</h2>' +
           '    <p class="page-subtitle">All accounts on this PDS</p>' +
           '  </div>' +
           '  <div class="header-actions">' +
           '    <input type="text" class="search-input" id="account-search" placeholder="Search by handle, DID, or email">' +
           '  </div>' +
           '</header>' +
           '<p class="filter-url-hint">' + (global.t ? global.t('common.search_in_url') : '') + '</p>' +
           '<div id="accounts-bulk-bar"></div>' +
           '<div class="table-card">' +
           '  <table class="data-table">' +
           '    <thead><tr>' +
           '      <th scope="col" aria-label="Bulk select"><span class="visually-hidden">Bulk select</span></th>' +
           '      <th>Handle</th><th>DID</th><th>Email</th><th>Created</th><th>Status</th><th>Actions</th>' +
           '    </tr></thead>' +
           '    <tbody id="accounts-table"></tbody>' +
           '  </table>' +
           '</div>';
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    const tbody = document.getElementById('accounts-table');
    if (!tbody) return;
    const search = document.getElementById('account-search');
    const q = search && search.value.trim();
    // Sync the query into the URL without a remount (replaceState) — a
    // remount would drop focus mid-typing in this live-search box. This is
    // why Accounts uses replace() rather than the FilterStrip pages' write().
    if (global.AuroraUrlState) global.AuroraUrlState.replace(q ? { q: q } : {});
    let accounts = [];
    try {
      const data = q
        ? await ep.atproto.searchAccounts({ q: q, limit: 50 })
        : await ep.atproto.listAccounts({ limit: 100 });
      accounts = (data && (data.accounts || data.users)) || [];
    } catch (e) {
      tbody.innerHTML = '<tr><td colspan="7"><div data-accounts-error></div></td></tr>';
      const cell = tbody.querySelector('[data-accounts-error]');
      global.AuroraErrorBoundary.mount(cell, {
        message: 'Failed to load accounts: ' + ((e && e.message) || 'unknown'),
        onRetry: refresh,
      });
      return;
    }

    if (accounts.length === 0) {
      tbody.innerHTML = '<tr><td colspan="7">' +
        (global.AuroraEmptyState ? global.AuroraEmptyState.render({
          icon: 'users', primary: 'No accounts match.', secondary: 'Try clearing your search.',
        }) : '<p class="empty-state">No accounts.</p>') + '</td></tr>';
      return;
    }
    const visible = new Set(accounts.map((a) => a.did));
    bulkSelected = new Set([...bulkSelected].filter((d) => visible.has(d)));

    tbody.innerHTML = accounts.map((u) => {
      const status = u.status || 'active';
      const created = global.AuroraTimestamp.render({ value: u.createdAt, context: 'detail' });
      return '<tr>' +
             '<td><input type="checkbox" class="bulk-select-account" data-did="' + esc(u.did) +
             '"' + (bulkSelected.has(u.did) ? ' checked' : '') +
             ' aria-label="Select ' + esc(u.handle) + '"></td>' +
             '<td><a href="#ops/accounts/' + encodeURIComponent(u.did) + '">@' + esc(u.handle || 'unknown') + '</a></td>' +
             '<td><code>' + esc(global.AuroraEntityRef ? global.AuroraEntityRef.shortDid(u.did) : u.did) + '</code></td>' +
             '<td>' + esc(u.email || 'N/A') + '</td>' +
             '<td>' + created + '</td>' +
             '<td>' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(status, status) : status) + '</td>' +
             '<td><a class="btn-sm btn-primary" href="#ops/accounts/' + encodeURIComponent(u.did) + '">View</a></td>' +
             '</tr>';
    }).join('');

    tbody.querySelectorAll('.bulk-select-account').forEach((cb) => {
      cb.addEventListener('change', (e) => {
        const did = e.target.dataset.did;
        if (e.target.checked) bulkSelected.add(did);
        else bulkSelected.delete(did);
        renderBulkBar();
      });
    });
    renderBulkBar();
  }

  function renderBulkBar() {
    const bar = document.getElementById('accounts-bulk-bar');
    if (!bar) return;
    const n = bulkSelected.size;
    if (n === 0) { bar.innerHTML = ''; return; }
    bar.innerHTML =
      '<div class="bulk-action-bar" role="toolbar" aria-label="Bulk actions for selected accounts">' +
      '<span><strong>' + n + '</strong> selected</span>' +
      '<button class="btn-sm btn-danger" id="bulk-takedown">Bulk takedown</button>' +
      '<button class="btn-sm btn-secondary" id="bulk-suspend">Bulk suspend</button>' +
      '<button class="btn-sm btn-secondary" id="bulk-restore">Bulk restore</button>' +
      '<button class="btn-sm btn-secondary" id="bulk-clear">Clear</button>' +
      '</div>';
    document.getElementById('bulk-takedown').addEventListener('click', () => openBulk('BatchTakedownAccounts'));
    document.getElementById('bulk-suspend').addEventListener('click', () => openBulk('BatchSuspendAccounts'));
    document.getElementById('bulk-restore').addEventListener('click', () => openBulk('BatchRestoreAccounts'));
    document.getElementById('bulk-clear').addEventListener('click', () => {
      bulkSelected = new Set();
      document.querySelectorAll('.bulk-select-account').forEach((cb) => { cb.checked = false; });
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
      availableActions: ['BatchTakedownAccounts', 'BatchSuspendAccounts', 'BatchRestoreAccounts', 'BatchApplyLabel', 'BatchRemoveLabel'],
      onCancel: () => handle.close(),
    });
    panel.mount(div);
    if (panel.state) panel.state.action = defaultAction;
    panel.render();
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  function debounce(fn, ms) {
    let t = null;
    return (...args) => { if (t) clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
  }

  if (global.AuroraRouter) global.AuroraRouter.register('opsAccounts', { mount: mount });
})(window);
