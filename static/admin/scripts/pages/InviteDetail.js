// Invite detail page (route: #ops/invites/:code).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.5.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const code = params && params.code;
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#ops/invites">Operations</a> <span class="breadcrumb-sep">›</span> <a href="#ops/invites">Invites</a> <span class="breadcrumb-sep">›</span> <code>' + esc(code) + '</code></nav>' +
      '<header class="page-header"><div><h2>Invite <code>' + esc(code) + '</code></h2></div></header>' +
      '<div id="id-body"><p class="empty-state">Loading…</p></div>';
    try {
      const data = await global.AuroraEndpoints.atproto.getInviteCodes({ codes: [code] });
      const codes = (data && (data.codes || data.inviteCodes)) || [];
      const c = codes.find((x) => x.code === code);
      if (!c) {
        document.getElementById('id-body').innerHTML = '<p class="empty-state">Invite code not found.</p>';
        return {};
      }
      renderBody(c);
    } catch (e) {
      document.getElementById('id-body').innerHTML =
        '<p class="empty-state">Could not load invite: ' + esc(e && e.message) + '</p>';
    }
    return {};
  }

  function renderBody(c) {
    const fmt = global.AuroraFormat;
    const usedBy = c.uses_array || c.usedBy || c.uses_list || [];
    const body = document.getElementById('id-body');
    body.innerHTML =
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Invite metadata</h3>' +
      '    <p><strong>Code:</strong> <code>' + esc(c.code) + '</code></p>' +
      '    <p><strong>Status:</strong> ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(c.disabled ? 'suspended' : 'active', c.disabled ? 'Disabled' : 'Active') : '') + '</p>' +
      '    <p><strong>Uses:</strong> ' + (c.uses || 0) + ' / ' + (c.available || 1) + '</p>' +
      '    <p><strong>Created at:</strong> ' + esc(fmt ? fmt.date(c.created_at, 'medium') : c.created_at) + '</p>' +
      '    <p><strong>Created by:</strong> ' + (c.created_by ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(c.created_by) : '@' + esc(c.created_by)) : 'system') + '</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Used by</h3>' +
      (usedBy.length === 0 ? '<p class="empty-state">No redemptions yet.</p>' :
       '<ul style="list-style:none; padding:0;">' + usedBy.map((u) =>
         '<li style="padding: 0.25rem 0;">' + (global.AuroraEntityRef ? global.AuroraEntityRef.account(u.usedBy || u.did, u.handle) : esc(u.usedBy || u.did || u)) +
         (u.usedAt ? ' <span style="color: var(--text-tertiary);">— ' + esc(fmt ? fmt.relativeTime(u.usedAt) : u.usedAt) + '</span>' : '') +
         '</li>').join('') + '</ul>') +
      '  </div>' +
      '</div>' +
      '<div class="settings-card" style="margin-top: 1rem;">' +
      '  <h3>Actions</h3>' +
      '  <button class="btn-danger" id="id-disable"' + (c.disabled ? ' disabled' : '') + '>' +
      (c.disabled ? 'Disabled' : 'Disable code') + '</button>' +
      '</div>';
    const btn = document.getElementById('id-disable');
    if (btn && !c.disabled) {
      btn.addEventListener('click', async () => {
        const result = await global.AuroraModal.destructiveConfirm({
          heading: 'Disable invite code',
          body: 'Disable invite code ' + c.code + '? It can be re-enabled later.',
          confirmLabel: 'Disable',
        });
        if (!result.confirmed) return;
        try {
          await global.AuroraEndpoints.atproto.disableInviteCode({ code: c.code });
          global.AuroraToast.success('Invite disabled.');
          if (global.AuroraRouter) global.AuroraRouter.navigate('ops/invites');
        } catch (e) {
          global.AuroraToast.danger('Disable failed: ' + (e && e.message ? e.message : ''));
        }
      });
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsInviteDetail', { mount: mount });
})(window);
