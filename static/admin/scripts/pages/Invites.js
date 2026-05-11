// Invites list page (route: #ops/invites).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.4.

(function (global) {
  'use strict';

  let bulkSelected = new Set();

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Invites</h2><p class="page-subtitle">Invite codes for this PDS</p></div>' +
      '  <div class="header-actions">' +
      '    <button class="btn-primary" id="generate-invites">Generate codes</button>' +
      '  </div>' +
      '</header>' +
      '<div class="invite-stats">' +
      '  <div class="stat-item"><span class="label">Total Codes:</span> <span class="value" id="invite-total">0</span></div>' +
      '  <div class="stat-item"><span class="label">Available:</span> <span class="value" id="invite-available">0</span></div>' +
      '  <div class="stat-item"><span class="label">Used:</span> <span class="value" id="invite-used">0</span></div>' +
      '</div>' +
      '<div id="invites-bulk-bar"></div>' +
      '<div class="table-card">' +
      '  <table class="data-table">' +
      '    <thead><tr>' +
      '      <th scope="col" aria-label="Bulk select"><span class="visually-hidden">Bulk select</span></th>' +
      '      <th>Code</th><th>Uses</th><th>Created By</th><th>Created At</th><th>Status</th><th>Actions</th>' +
      '    </tr></thead>' +
      '    <tbody id="invites-table"></tbody>' +
      '  </table>' +
      '</div>';
    bulkSelected = new Set();
    document.getElementById('generate-invites').addEventListener('click', generateInvites);
    await refresh();
    return { unmount: () => { bulkSelected = new Set(); } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    const tbody = document.getElementById('invites-table');
    if (!tbody || !ep) return;
    try {
      const data = await ep.atproto.listInviteCodes({ limit: 100 });
      const codes = (data && data.codes) || [];
      const visible = new Set(codes.map((c) => c.code));
      bulkSelected = new Set([...bulkSelected].filter((c) => visible.has(c)));
      const total = codes.length;
      const available = codes.filter((c) => !c.disabled && (c.uses || 0) < (c.available || 1)).length;
      const used = codes.filter((c) => (c.uses || 0) >= (c.available || 1)).length;
      document.getElementById('invite-total').textContent = String(total);
      document.getElementById('invite-available').textContent = String(available);
      document.getElementById('invite-used').textContent = String(used);

      if (codes.length === 0) {
        tbody.innerHTML = '<tr><td colspan="7">' +
          (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'ticket', primary: 'No invite codes yet.' }) :
           '<p class="empty-state">No invite codes.</p>') + '</td></tr>';
        renderBulkBar();
        return;
      }
      const fmt = global.AuroraFormat;
      tbody.innerHTML = codes.map((c) =>
        '<tr>' +
        '<td><input type="checkbox" class="bulk-select-invite" data-code="' + esc(c.code) + '"' +
        (bulkSelected.has(c.code) ? ' checked' : '') +
        (c.disabled ? ' disabled aria-label="Already disabled"' : ' aria-label="Select invite code"') + '></td>' +
        '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.invite(c.code) : '<code>' + esc(c.code) + '</code>') + '</td>' +
        '<td>' + (c.uses || 0) + ' / ' + (c.available || 1) + '</td>' +
        '<td>' + (c.created_by ? '@' + esc(c.created_by) : 'system') + '</td>' +
        '<td>' + esc(fmt ? fmt.date(c.created_at, 'short') : c.created_at) + '</td>' +
        '<td>' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(c.disabled ? 'suspended' : 'active', c.disabled ? 'Disabled' : 'Active') : '') + '</td>' +
        '<td><button class="btn-sm btn-danger" data-disable="' + esc(c.code) + '"' +
            (c.disabled ? ' disabled' : '') + '>Disable</button></td>' +
        '</tr>'
      ).join('');
      tbody.querySelectorAll('.bulk-select-invite').forEach((cb) => {
        cb.addEventListener('change', (e) => {
          const code = e.target.dataset.code;
          if (e.target.checked) bulkSelected.add(code);
          else bulkSelected.delete(code);
          renderBulkBar();
        });
      });
      tbody.querySelectorAll('[data-disable]').forEach((btn) => {
        btn.addEventListener('click', () => disableInvite(btn.dataset.disable));
      });
      renderBulkBar();
    } catch (e) {
      tbody.innerHTML = '<tr><td colspan="7"><p class="empty-state">Could not load invites: ' +
                        esc(e && e.message) + '</p></td></tr>';
    }
  }

  function renderBulkBar() {
    const bar = document.getElementById('invites-bulk-bar');
    if (!bar) return;
    const n = bulkSelected.size;
    if (n === 0) { bar.innerHTML = ''; return; }
    bar.innerHTML =
      '<div class="bulk-action-bar" role="toolbar">' +
      '<span><strong>' + n + '</strong> code' + (n === 1 ? '' : 's') + ' selected</span>' +
      '<button class="btn-sm btn-danger" id="ib-disable">Disable selected</button>' +
      '<button class="btn-sm btn-secondary" id="ib-clear">Clear</button>' +
      '</div>';
    document.getElementById('ib-disable').addEventListener('click', bulkDisable);
    document.getElementById('ib-clear').addEventListener('click', () => {
      bulkSelected = new Set();
      document.querySelectorAll('.bulk-select-invite').forEach((cb) => { cb.checked = false; });
      renderBulkBar();
    });
  }

  async function bulkDisable() {
    const codes = [...bulkSelected];
    if (codes.length === 0) return;
    const plural = codes.length === 1 ? '' : 's';
    const result = await global.AuroraModal.destructiveConfirm({
      heading: 'Disable invite codes',
      body: 'Disable ' + codes.length + ' invite code' + plural + '? They can be re-enabled later.',
      confirmLabel: 'Disable all',
    });
    if (!result.confirmed) return;
    try {
      await global.AuroraEndpoints.atproto.disableInviteCodes({ codes: codes });
      global.AuroraToast.success('Disabled ' + codes.length + ' code' + plural + '.');
      bulkSelected = new Set();
      await refresh();
    } catch (e) {
      global.AuroraToast.danger('Bulk disable failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function disableInvite(code) {
    const result = await global.AuroraModal.destructiveConfirm({
      heading: 'Disable invite code',
      body: 'Disable invite code ' + code + '? It can be re-enabled later.',
      confirmLabel: 'Disable',
    });
    if (!result.confirmed) return;
    try {
      await global.AuroraEndpoints.atproto.disableInviteCode({ code: code });
      global.AuroraToast.success('Invite code disabled.');
      await refresh();
    } catch (e) {
      global.AuroraToast.danger('Failed to disable: ' + (e && e.message ? e.message : ''));
    }
  }

  async function generateInvites() {
    const div = document.createElement('div');
    div.innerHTML =
      '<div class="form-group"><label>How many to generate?</label>' +
      '<input type="number" id="gi-count" value="10" min="1" max="100"></div>' +
      '<div class="form-group"><label>Optional: bind to account DID</label>' +
      '<input type="text" id="gi-account" placeholder="did:plc:..."></div>' +
      '<div class="action-panel-buttons">' +
      '<button class="btn-secondary" id="gi-cancel">Cancel</button>' +
      '<button class="btn-primary" id="gi-submit">Generate</button>' +
      '</div>';
    const modal = global.AuroraModal.open({ title: 'Generate invite codes', body: div });
    div.querySelector('#gi-cancel').addEventListener('click', () => modal.close());
    div.querySelector('#gi-submit').addEventListener('click', async () => {
      const count = parseInt(div.querySelector('#gi-count').value, 10);
      const account = div.querySelector('#gi-account').value.trim();
      if (!count || count <= 0) { global.AuroraToast.warning('Specify a positive count.'); return; }
      modal.close();
      let generated = 0;
      let failed = 0;
      for (let i = 0; i < count; i++) {
        try {
          const body = { uses: 1 };
          if (account) body.forAccount = account;
          await global.AuroraEndpoints.atproto.createInviteCode(body);
          generated++;
        } catch (e) { failed++; }
      }
      if (generated > 0) global.AuroraToast.success('Generated ' + generated + ' invite code' + (generated === 1 ? '' : 's') + '.');
      if (failed > 0) global.AuroraToast.warning('Failed to generate ' + failed + ' code' + (failed === 1 ? '' : 's') + '.');
      await refresh();
    });
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsInvites', { mount: mount });
})(window);
