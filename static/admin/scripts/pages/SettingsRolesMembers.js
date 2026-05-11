// Settings → Roles members list (route: #settings/roles/:role).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.4.

(function (global) {
  'use strict';

  // Map UI tier slugs (plural; derived from role.name in
  // SettingsRoles.groupRoles and embedded in route URLs like
  // #settings/roles/moderators, which this page reads as
  // params.role) to canonical Role enum strings (singular,
  // lowercase) per src/admin/roles.rs:67-78 `Role::from_str`.
  // The backend's grantRole / revokeRole handlers call
  // .parse::<Role>() on the wire `role` field; passing the
  // plural slug returns Validation("Invalid role: moderators")
  // and a 400.
  //
  // Duplicated verbatim in pages/SettingsRoles.js per Arc 6's
  // anti-restructuring convention (see the parallel
  // `settingSourceSuffix` duplication from Step 2).
  function tierToRoleString(tier) {
    switch (tier) {
      case 'moderators':     return 'moderator';
      case 'administrators': return 'admin';
      case 'superadmins':    return 'superadmin';
      default:               return tier;
    }
  }

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
    // Map the route's plural tier slug to the canonical Role enum
    // string the server's `Role::from_str` accepts. See
    // tierToRoleString comment above.
    const wireRole = tierToRoleString(role);
    try {
      const res = await global.AuroraEndpoints.superadmin.revokeRole({ did: did, role: wireRole, rationale: result.rationale });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Role revoked.', auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      if (global.AuroraRouter) global.AuroraRouter.dispatch();
    } catch (e) {
      // Surface err.message directly so Step 1's translation layer
      // (or the fallback 'HTTP <status>: <msg>' rendering) shows
      // through. The prior 'Revoke failed: <msg>' hand-prefix
      // shadowed the translated prose for recognized error codes.
      global.AuroraToast.danger(e && e.message ? e.message : 'Revoke failed.');
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
    // Map the route's plural tier slug to the canonical Role enum
    // string. Same fix as the revoke path above.
    const wireRole = tierToRoleString(role);
    try {
      const res = await global.AuroraEndpoints.superadmin.grantRole({
        did: result.values.did,
        role: wireRole,
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
