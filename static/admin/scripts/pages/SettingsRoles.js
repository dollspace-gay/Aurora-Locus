// Settings → Roles page (route: #settings/roles).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.3.

(function (global) {
  'use strict';

  async function mount({ container }) {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#settings/roles">Settings</a> <span class="breadcrumb-sep">›</span> Roles</nav>' +
      '<header class="page-header"><div><h2>Roles</h2><p class="page-subtitle">Authority tiers and current members</p></div></header>' +
      '<div id="roles-list"><p class="empty-state">Loading…</p></div>';
    try {
      const data = await global.AuroraEndpoints.atproto.listRoles();
      renderRoles(data, isSuper);
    } catch (e) {
      document.getElementById('roles-list').innerHTML =
        '<p class="empty-state">Could not load roles: ' + esc(e && e.message) + '</p>';
    }
    return {};
  }

  function renderRoles(data, isSuper) {
    const roles = (data && data.roles) || [
      { name: 'Moderators', members: [], description: 'Acts on subjects-as-content' },
      { name: 'Administrators', members: [], description: 'Acts on accounts-as-infrastructure' },
      { name: 'SuperAdmins', members: [], description: 'Acts on authority itself' },
    ];
    const c = document.getElementById('roles-list');
    c.innerHTML = roles.map((role) => {
      const memberCount = (role.members || role.members_count || 0) + (Array.isArray(role.members) ? 0 : 0);
      const members = Array.isArray(role.members) ? role.members : [];
      const memberSlug = String(role.name || role.role || '').toLowerCase().replace(/\s+/g, '-');
      return '<div class="role-card">' +
             '  <h3>' + esc(role.name || role.role || 'Role') + ' <small style="font-weight:normal; color: var(--text-secondary);">[' + (memberCount || members.length) + ' member' + ((memberCount || members.length) === 1 ? '' : 's') + ']</small></h3>' +
             (role.description ? '<p class="settings-help">' + esc(role.description) + '</p>' : '') +
             '<div class="role-members">' +
             (members.length === 0 ? '<p class="settings-help">No members.</p>' :
              members.slice(0, 12).map((m) => global.AuroraEntityRef ? global.AuroraEntityRef.account(m.did, m.handle) : '@' + esc(m.handle || m.did)).join(' ')) +
             '</div>' +
             '<div class="action-panel-buttons" style="justify-content: flex-start; gap: 0.5rem;">' +
             (members.length > 12 ? '<a class="btn-sm btn-secondary" href="#settings/roles/' + encodeURIComponent(memberSlug) + '">View all</a>' : '') +
             (isSuper ? '<button class="btn-sm btn-primary" data-grant="' + esc(memberSlug) + '">Grant role</button>' : '') +
             '</div>' +
             '</div>';
    }).join('');
    if (isSuper) {
      c.querySelectorAll('[data-grant]').forEach((btn) => {
        btn.addEventListener('click', () => openGrantModal(btn.dataset.grant));
      });
    }
  }

  function openGrantModal(roleSlug) {
    const div = document.createElement('div');
    div.innerHTML =
      '<div class="form-group"><label>Account DID or handle</label><input type="text" id="grant-target"></div>' +
      '<div class="form-group"><label>Rationale (required)</label><textarea id="grant-r" rows="2" style="width:100%;"></textarea></div>' +
      '<div class="action-panel-buttons">' +
      '  <button class="btn-secondary" id="grant-cancel">Cancel</button>' +
      '  <button class="btn-primary" id="grant-submit">Grant role</button>' +
      '</div>';
    const handle = global.AuroraModal.open({ title: 'Grant ' + roleSlug + ' role', body: div });
    div.querySelector('#grant-cancel').addEventListener('click', () => handle.close());
    div.querySelector('#grant-submit').addEventListener('click', async () => {
      const target = div.querySelector('#grant-target').value.trim();
      const rationale = div.querySelector('#grant-r').value.trim();
      if (!target || !rationale) { global.AuroraToast.warning('Target and rationale required.'); return; }
      try {
        await global.AuroraEndpoints.superadmin.grantRole({ subject: target, role: roleSlug, rationale: rationale });
        global.AuroraToast.success('Role granted.');
        handle.close();
        await mountReload();
      } catch (e) {
        global.AuroraToast.danger('Grant failed: ' + (e && e.message ? e.message : ''));
      }
    });
  }

  async function mountReload() {
    if (global.AuroraRouter) global.AuroraRouter.dispatch();
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('settingsRoles', { mount: mount });
})(window);
