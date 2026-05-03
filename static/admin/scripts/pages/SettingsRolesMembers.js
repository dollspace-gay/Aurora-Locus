// Settings → Roles members list (route: #settings/roles/:role).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.4.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const role = params && params.role;
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#settings/roles">Settings</a> <span class="breadcrumb-sep">›</span> <a href="#settings/roles">Roles</a> <span class="breadcrumb-sep">›</span> ' + esc(role) + '</nav>' +
      '<header class="page-header">' +
      '  <div><h2>' + esc(role) + ' members</h2></div>' +
      (isSuper ? '<div class="header-actions"><button class="btn-primary" id="rmm-grant">Grant role</button></div>' : '') +
      '</header>' +
      '<div class="table-card">' +
      '  <table class="data-table">' +
      '    <thead><tr><th>Handle</th><th>DID</th><th>Granted</th><th>Granted by</th><th>Actions</th></tr></thead>' +
      '    <tbody id="rmm-table"><tr><td colspan="5"><p class="empty-state">Loading…</p></td></tr></tbody>' +
      '  </table>' +
      '</div>';
    if (isSuper) {
      document.getElementById('rmm-grant').addEventListener('click', () => openGrant(role));
    }
    await refresh(role, isSuper);
    return {};
  }

  async function refresh(role, isSuper) {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.atproto.listRoles({ role: role, limit: 100 });
      const allRoles = (data && data.roles) || [];
      const target = allRoles.find((r) => normalize(r.name || r.role) === normalize(role));
      const members = target ? (target.members || []) : (data && data.members) || [];
      const tbody = document.getElementById('rmm-table');
      if (members.length === 0) {
        tbody.innerHTML = '<tr><td colspan="5">' +
          (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'users', primary: 'No members yet.' }) :
           '<p class="empty-state">No members.</p>') + '</td></tr>';
        return;
      }
      const fmt = global.AuroraFormat;
      tbody.innerHTML = members.map((m) =>
        '<tr>' +
        '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.account(m.did, m.handle) : '@' + esc(m.handle || m.did)) + '</td>' +
        '<td><code>' + esc(global.AuroraEntityRef ? global.AuroraEntityRef.shortDid(m.did) : m.did) + '</code></td>' +
        '<td>' + esc(fmt && m.grantedAt ? fmt.date(m.grantedAt, 'short') : '—') + '</td>' +
        '<td>' + (m.grantedBy ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(m.grantedBy) : esc(m.grantedBy)) : '—') + '</td>' +
        '<td>' + (isSuper ? '<button class="btn-sm btn-danger" data-revoke="' + esc(m.did) + '">Revoke</button>' : '—') + '</td>' +
        '</tr>'
      ).join('');
      if (isSuper) {
        tbody.querySelectorAll('[data-revoke]').forEach((btn) => {
          btn.addEventListener('click', () => revoke(role, btn.dataset.revoke));
        });
      }
    } catch (e) {
      const tbody = document.getElementById('rmm-table');
      if (tbody) tbody.innerHTML = '<tr><td colspan="5"><p class="empty-state">Could not load members: ' + esc(e && e.message) + '</p></td></tr>';
    }
  }

  async function revoke(role, did) {
    const rationale = prompt('Rationale (required, recorded in audit log):');
    if (!rationale) return;
    try {
      await global.AuroraEndpoints.superadmin.revokeRole({ subject: did, role: role, rationale: rationale });
      global.AuroraToast.success('Role revoked.');
      if (global.AuroraRouter) global.AuroraRouter.dispatch();
    } catch (e) {
      global.AuroraToast.danger('Revoke failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function openGrant(role) {
    const div = document.createElement('div');
    div.innerHTML =
      '<div class="form-group"><label>Account DID or handle</label><input type="text" id="rmm-grant-target"></div>' +
      '<div class="form-group"><label>Rationale (required)</label><textarea id="rmm-grant-r" rows="2" style="width:100%;"></textarea></div>' +
      '<div class="action-panel-buttons">' +
      '  <button class="btn-secondary" id="rmm-grant-cancel">Cancel</button>' +
      '  <button class="btn-primary" id="rmm-grant-submit">Grant role</button>' +
      '</div>';
    const handle = global.AuroraModal.open({ title: 'Grant ' + role + ' role', body: div });
    div.querySelector('#rmm-grant-cancel').addEventListener('click', () => handle.close());
    div.querySelector('#rmm-grant-submit').addEventListener('click', async () => {
      const target = div.querySelector('#rmm-grant-target').value.trim();
      const rationale = div.querySelector('#rmm-grant-r').value.trim();
      if (!target || !rationale) { global.AuroraToast.warning('Target and rationale required.'); return; }
      try {
        await global.AuroraEndpoints.superadmin.grantRole({ subject: target, role: role, rationale: rationale });
        global.AuroraToast.success('Role granted.');
        handle.close();
        if (global.AuroraRouter) global.AuroraRouter.dispatch();
      } catch (e) {
        global.AuroraToast.danger('Grant failed: ' + (e && e.message ? e.message : ''));
      }
    });
  }

  function normalize(s) { return String(s || '').toLowerCase().replace(/s$/, ''); }
  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('settingsRolesMembers', { mount: mount });
})(window);
