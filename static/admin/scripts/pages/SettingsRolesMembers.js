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
    // Canonical destructive-confirm example per V04_DESIGN §5.3.3:
    // REVOKE typed gate + required rationale. The role + target are
    // surfaced in the heading; the operator types REVOKE to unlock
    // submit and supplies the rationale that lands in the audit log.
    const result = await global.AuroraModal.destructiveConfirm({
      heading: 'Revoke ' + role + ' role from ' + did,
      body: 'This action will be recorded in the audit trail.',
      typedConfirmGate: 'REVOKE',
      rationaleRequired: true,
      confirmLabel: 'Revoke role',
    });
    if (!result.confirmed) return;
    try {
      const res = await global.AuroraEndpoints.superadmin.revokeRole({ did: did, role: role, rationale: result.rationale });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Role revoked.', auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      if (global.AuroraRouter) global.AuroraRouter.dispatch();
    } catch (e) {
      global.AuroraToast.danger('Revoke failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function openGrant(role) {
    // Mirror of SettingsRoles.js:openGrantModal. The members page
    // exposes its own "Grant role" entry point on the per-role
    // detail view; both flows go through the same backend wire
    // contract { did, role, rationale }. See V04_DESIGN §5.4.5
    // and the cross-page comment in SettingsRoles.js.
    const result = await global.AuroraModal.form({
      heading: 'Grant ' + role + ' role',
      body: 'Grant this role to a member by DID. The grant lands as one audit-chain entry.',
      fields: [
        {
          name: 'did',
          label: 'DID',
          type: 'text',
          required: true,
          placeholder: 'did:plc:…',
          validate: (value) => {
            if (!value || !value.startsWith('did:')) {
              return 'DID must start with "did:" (e.g., did:plc:…).';
            }
            return null;
          },
        },
        {
          name: 'rationale',
          label: 'Rationale (recorded in audit log)',
          type: 'textarea',
          required: true,
        },
      ],
      submitLabel: 'Grant role',
    });
    if (!result.submitted) return;
    try {
      const res = await global.AuroraEndpoints.superadmin.grantRole({
        did: result.values.did,
        role: role,
        rationale: result.values.rationale,
      });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Granted ' + role + ' role to ' + result.values.did + '.', auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      if (global.AuroraRouter) global.AuroraRouter.dispatch();
    } catch (e) {
      global.AuroraToast.danger(e && e.message ? e.message : 'Grant failed.');
    }
  }

  function normalize(s) { return String(s || '').toLowerCase().replace(/s$/, ''); }
  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('settingsRolesMembers', { mount: mount });
})(window);
