// Settings → Roles page (route: #settings/roles).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.3.

(function (global) {
  'use strict';

  // Map UI tier slugs (plural, derived from role.name in groupRoles
  // and embedded in route URLs like #settings/roles/moderators) to
  // canonical Role enum strings (singular, lowercase) per
  // src/admin/roles.rs:67-78 `Role::from_str`. The backend's
  // grantRole / revokeRole handlers call .parse::<Role>() on the
  // wire `role` field; passing the plural slug returns
  // `Validation("Invalid role: moderators")` and a 400.
  //
  // Duplicated verbatim in pages/SettingsRolesMembers.js — the
  // codebase doesn't have a cross-page utility location for view
  // helpers, and manufacturing a module just for two callers is
  // over-investment per Arc 6's anti-restructuring convention
  // (see the parallel `settingSourceSuffix` duplication from
  // Step 2).
  //
  // Unknown tier strings pass through unchanged so a future-added
  // tier doesn't silently lose its wire form before the helper is
  // updated — the server will reject with its native "Invalid
  // role" message in that case.
  function tierToRoleString(tier) {
    switch (tier) {
      case 'moderators':     return 'moderator';
      case 'administrators': return 'admin';
      case 'superadmins':    return 'superadmin';
      default:               return tier;
    }
  }

  async function mount({ container }) {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#settings/roles">Settings</a> <span class="breadcrumb-sep">›</span> Roles</nav>' +
      '<header class="page-header"><div><h2>Roles</h2><p class="page-subtitle">Authority tiers and current members</p></div></header>' +
      '<div id="roles-list"><p class="empty-state">Loading…</p></div>';
    try {
      const data = await global.AuroraEndpoints.atproto.listRoles();
      renderRoles(groupRoles(data), isSuper);
    } catch (e) {
      document.getElementById('roles-list').innerHTML =
        '<p class="empty-state">Could not load roles: ' + esc(e && e.message) + '</p>';
    }
    return {};
  }

  // The com.atproto.admin.listRoles handler returns a flat array of
  // active assignment rows: {roles: [{did, role, granted_by, ...}, ...]}.
  // The page renders authority *tiers* (Moderators/Administrators/
  // SuperAdmins) with their members, so group the flat assignments by
  // role tier here. If the server ever pre-groups (members[] present on
  // the first entry), pass it through unchanged.
  function groupRoles(data) {
    const flat = (data && Array.isArray(data.roles)) ? data.roles : [];
    if (flat.length > 0 && Array.isArray(flat[0].members)) {
      return flat;
    }
    const tiers = [
      { key: 'moderator', name: 'Moderators', description: 'Acts on subjects-as-content', members: [] },
      { key: 'admin', name: 'Administrators', description: 'Acts on accounts-as-infrastructure', members: [] },
      { key: 'superadmin', name: 'SuperAdmins', description: 'Acts on authority itself', members: [] },
    ];
    for (const row of flat) {
      const tier = tiers.find((t) => t.key === row.role);
      if (tier) tier.members.push({ did: row.did, handle: row.handle });
    }
    return tiers;
  }

  function renderRoles(roles, isSuper) {
    const c = document.getElementById('roles-list');
    c.innerHTML = roles.map((role) => {
      const members = Array.isArray(role.members) ? role.members : [];
      const memberCount = members.length;
      const memberSlug = String(role.name || role.role || '').toLowerCase().replace(/\s+/g, '-');
      return '<div class="role-card">' +
             '  <h3>' + esc(role.name || role.role || 'Role') + ' <small style="font-weight:normal; color: var(--text-secondary);">[' + memberCount + ' member' + (memberCount === 1 ? '' : 's') + ']</small></h3>' +
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

  async function openGrantModal(roleSlug) {
    // Per V04_DESIGN §5.4.5. The form collects the target DID and a
    // required audit-log rationale. DID prefix is validated
    // client-side; full DID format is server-authoritative (the
    // backend rejects invalid DIDs with a 4xx). The kickoff
    // suggested optional `notes` / `force` fields, but the backend
    // `GrantRoleRequest` (src/api/admin.rs:807-812) accepts only
    // { did, role, rationale } — added fields would be ignored
    // (or rejected with deny_unknown_fields), so the form mirrors
    // the wire contract.
    const result = await global.AuroraModal.form({
      heading: 'Grant ' + roleSlug + ' role',
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
    // Map the tier slug (plural display form derived from role.name)
    // to the canonical Role enum string (singular) the server's
    // `Role::from_str` accepts. See tierToRoleString comment above.
    const wireRole = tierToRoleString(roleSlug);
    try {
      const res = await global.AuroraEndpoints.superadmin.grantRole({
        did: result.values.did,
        role: wireRole,
        rationale: result.values.rationale,
      });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Granted ' + roleSlug + ' role to ' + result.values.did + '.', auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      await mountReload();
    } catch (e) {
      // err.message carries Step 1's translated message when the
      // server returns a structured error code; bare HTTP status
      // otherwise. No need to hand-prefix "Grant failed:" — the
      // translation layer or fallback rendering does the right
      // thing.
      global.AuroraToast.danger(e && e.message ? e.message : 'Grant failed.');
    }
  }

  async function mountReload() {
    if (global.AuroraRouter) global.AuroraRouter.dispatch();
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('settingsRoles', { mount: mount });
})(window);
