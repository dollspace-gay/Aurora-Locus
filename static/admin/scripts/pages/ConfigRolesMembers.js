// Configuration → Roles members list (route: #configuration/roles/:role).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.4.

(function (global) {
  'use strict';

  function T(key, params) { return global.t ? global.t(key, params) : key; }

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
      '<nav class="breadcrumb"><a href="#configuration/roles">' + esc(T('settings.roles.crumb')) + '</a> <span class="breadcrumb-sep">›</span> <a href="#configuration/roles">' + esc(T('settings.roles.title')) + '</a> <span class="breadcrumb-sep">›</span> ' + esc(role) + '</nav>' +
      '<header class="page-header">' +
      '  <div><h2>' + esc(T('settings.roles.members_title', { role: role })) + '</h2></div>' +
      (isSuper ? '<div class="header-actions"><button class="btn-primary" id="rmm-grant">' + esc(T('settings.roles.grant')) + '</button></div>' : '') +
      '</header>' +
      '<div class="table-card">' +
      '  <table class="data-table">' +
      '    <thead><tr><th>' + esc(T('settings.roles.col_account')) + '</th><th>' + esc(T('settings.roles.col_did')) + '</th><th>' + esc(T('settings.roles.col_granted')) + '</th><th>' + esc(T('settings.roles.col_granted_by')) + '</th><th>' + esc(T('settings.roles.col_actions')) + '</th></tr></thead>' +
      '    <tbody id="rmm-table"><tr><td colspan="5">' + global.AuroraSkeleton.lines(3) + '</td></tr></tbody>' +
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
    // Operator's own DID — drives the self-revoke guard (§9.1). Falls back
    // to '' (no match) until the session hydrates; the server-side check is
    // the authoritative guardrail, this is a UX nudge. Mirrors the inline
    // guard in ConfigRoles.renderMemberRow (duplicated per the codebase's
    // anti-restructuring convention, like tierToRoleString above, rather
    // than a shared helper).
    const session = global.AuroraSession;
    const operatorDid = (session && session.user() && session.user().did) || '';
    const tbody = document.getElementById('rmm-table');
    try {
      // Canonical wire shape (§9.1): com.atproto.admin.listRoles returns a
      // FLAT list of active assignments — {roles: [{did, role, granted_by,
      // granted_at, ...}]} in snake_case (src/admin/roles.rs AdminRole),
      // with NO per-row handle and NO nested members. The handler also
      // ignores the `role` query param (it gates only on `did`), so filter
      // by tier here, client-side. ConfigRoles reads the same flat shape.
      // Backend gaps (handle enrichment, honoring the role filter) are
      // tracked as separate backend tickets — see #203.
      const data = await ep.atproto.listRoles({ role: role, limit: 100 });
      const allRoles = (data && data.roles) || [];
      // Map the plural tier slug from the route / the list page's "View
      // members" link (moderators / administrators / superadmins) to the
      // canonical Role string, then match. tierToRoleString handles
      // "administrators" → "admin"; a trailing-s strip would not.
      const wantRole = String(tierToRoleString(role)).toLowerCase();
      const members = allRoles.filter((r) => String(r.role).toLowerCase() === wantRole);
      if (members.length === 0) {
        tbody.innerHTML = '<tr><td colspan="5">' +
          (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'users', primary: T('settings.roles.no_members_yet') }) :
           '<p class="empty-state">' + esc(T('settings.roles.no_members_yet')) + '</p>') + '</td></tr>';
        return;
      }
      const fmt = global.AuroraFormat;
      const ref = global.AuroraEntityRef;
      tbody.innerHTML = members.map((m) => {
        const isSelf = operatorDid && m.did === operatorDid;
        return '<tr>' +
          '<td>' + (ref ? ref.account(m.did) : '@' + esc(m.did)) + '</td>' +
          '<td><code>' + esc(ref ? ref.shortDid(m.did) : m.did) + '</code></td>' +
          '<td>' + global.AuroraTimestamp.render({ value: m.granted_at, context: 'detail' }) + '</td>' +
          '<td>' + (m.granted_by ? (ref ? ref.account(m.granted_by) : esc(m.granted_by)) : '—') + '</td>' +
          '<td>' + (isSuper
            ? '<button class="btn-sm btn-danger" data-revoke="' + esc(m.did) + '"' +
              (isSelf ? ' disabled title="' + esc(T('settings.roles.self_revoke_tooltip')) + '"' : '') + '>' + esc(T('settings.roles.revoke')) + '</button>'
            : '—') + '</td>' +
          '</tr>';
      }).join('');
      if (isSuper) {
        tbody.querySelectorAll('[data-revoke]').forEach((btn) => {
          if (btn.disabled) return;
          btn.addEventListener('click', () => revoke(role, btn.dataset.revoke));
        });
      }
    } catch (e) {
      if (tbody) {
        tbody.innerHTML = '<tr><td colspan="5"><div data-members-error></div></td></tr>';
        const cell = tbody.querySelector('[data-members-error]');
        global.AuroraInlineError.mount(cell, {
          message: T('settings.roles.could_not_load_members', { message: (e && e.message) || '' }),
          onRetry: function () { refresh(role, isSuper); },
        });
      }
    }
  }

  async function revoke(role, did) {
    // Canonical destructive-confirm example per V04_DESIGN §5.3.3:
    // REVOKE typed gate + required rationale. The role + target are
    // surfaced in the heading; the operator types REVOKE to unlock
    // submit and supplies the rationale that lands in the audit log.
    const result = await global.AuroraModal.destructiveConfirm({
      heading: T('settings.roles.revoke_heading', { role: role, did: did }),
      body: T('settings.roles.revoke_body'),
      typedConfirmGate: 'REVOKE',
      rationaleRequired: true,
      confirmLabel: T('settings.roles.revoke_confirm'),
    });
    if (!result.confirmed) return;
    // Map the route's plural tier slug to the canonical Role enum
    // string the server's `Role::from_str` accepts. See
    // tierToRoleString comment above.
    const wireRole = tierToRoleString(role);
    try {
      const res = await global.AuroraEndpoints.superadmin.revokeRole({ did: did, role: wireRole, rationale: result.rationale });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success(T('settings.roles.revoke_success'), auditEntryId ? {
        action: {
          label: T('settings.roles.view_audit'),
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      if (global.AuroraRouter) global.AuroraRouter.dispatch();
    } catch (e) {
      // Surface err.message directly so Step 1's translation layer
      // (or the fallback 'HTTP <status>: <msg>' rendering) shows
      // through. The prior 'Revoke failed: <msg>' hand-prefix
      // shadowed the translated prose for recognized error codes.
      global.AuroraToast.danger(e && e.message ? e.message : T('settings.roles.revoke_failed'));
    }
  }

  async function openGrant(role) {
    // Mirror of SettingsRoles.js:openGrantModal. The members page
    // exposes its own "Grant role" entry point on the per-role
    // detail view; both flows go through the same backend wire
    // contract { did, role, rationale }. See V04_DESIGN §5.4.5
    // and the cross-page comment in SettingsRoles.js.
    const result = await global.AuroraModal.form({
      heading: T('settings.roles.grant_heading', { role: role }),
      body: T('settings.roles.grant_body'),
      fields: [
        {
          name: 'did',
          label: T('settings.roles.field_did'),
          type: 'text',
          required: true,
          placeholder: T('settings.roles.did_placeholder'),
          validate: (value) => {
            if (!value || !value.startsWith('did:')) {
              return T('settings.roles.did_invalid');
            }
            return null;
          },
        },
        {
          name: 'rationale',
          label: T('settings.roles.field_rationale'),
          type: 'textarea',
          required: true,
        },
      ],
      submitLabel: T('settings.roles.grant_submit'),
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
      global.AuroraToast.success(T('settings.roles.grant_success', { role: role, did: result.values.did }), auditEntryId ? {
        action: {
          label: T('settings.roles.view_audit'),
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      if (global.AuroraRouter) global.AuroraRouter.dispatch();
    } catch (e) {
      global.AuroraToast.danger(e && e.message ? e.message : T('settings.roles.grant_failed'));
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('configRolesMembers', { mount: mount });
})(window);
