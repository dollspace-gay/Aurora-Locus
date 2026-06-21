// Account detail page (route: #ops/accounts/:did).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.2 — the canonical per-account
// surface. Two-column layout: drawers (Account overview, Moderation
// actions, Account management, Subject history) on the left, context
// rail (Subject context, Records authored, Blob inventory, Invite
// lineage) on the right. Drawer-level role gating per §5.2.4.

(function (global) {
  'use strict';

  function T(key, params) { return global.t ? global.t(key, params) : key; }

  let currentDid = null;
  let currentAccount = null;

  async function mount({ params, container }) {
    const did = params && params.did;
    if (!did) {
      container.innerHTML = '<p class="empty-state">Missing DID parameter.</p>';
      return {};
    }
    currentDid = did;
    container.innerHTML = renderShell(did);
    await loadAccount();
    await Promise.all([
      loadSubjectContext(),
      loadSubjectHistory(),
      loadRecords(),
      loadBlobs(),
      loadInvites(),
      loadRoles(),
    ]);
    return { unmount: () => { currentDid = null; currentAccount = null; } };
  }

  function renderShell(did) {
    // Subject-context pivot to the Audit page pre-filtered to this DID
    // (§6.5 + §9.8 audit cross-pivot convention, #264). The Audit route is
    // Moderator+ (routes.js), so the affordance only renders for operators
    // who can actually follow it — a gated dead link is worse than none.
    // The filter rides the canonical `#mod/audit?subject=<did>` URL-state
    // form (router splits path?query; Audit.mount reads `subject`).
    const session = global.AuroraSession;
    const auditPivot = (session && session.hasRole('moderator'))
      ? '  <p class="detail-header-pivots"><a href="#mod/audit?subject=' +
        encodeURIComponent(did) + '">' + esc(T('accountDetail.viewAuditChain')) + '</a></p>'
      : '';
    return '<nav class="breadcrumb" aria-label="Breadcrumb">' +
           '  <a href="#ops/accounts">Operations</a>' +
           '  <span class="breadcrumb-sep">›</span>' +
           '  <a href="#ops/accounts">Accounts</a>' +
           '  <span class="breadcrumb-sep">›</span>' +
           '  <span id="ad-handle-bc">' + esc(did) + '</span>' +
           '</nav>' +
           '<header class="detail-header">' +
           '  <h2 id="ad-handle">Loading…</h2>' +
           '  <p class="meta" id="ad-meta"><code>' + esc(did) + '</code></p>' +
           auditPivot +
           '</header>' +
           '<div class="detail-layout">' +
           '  <div class="detail-primary" id="ad-primary"></div>' +
           '  <aside class="detail-rail" id="ad-rail" aria-label="Context"></aside>' +
           '</div>';
  }

  async function loadAccount() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    let info;
    try {
      try {
        info = await ep.atproto.getAccountInfo(currentDid);
      } catch (e) {
        info = await ep.atproto.getAccount(currentDid);
      }
    } catch (e) {
      const primary = document.getElementById('ad-primary');
      if (primary) global.AuroraErrorBoundary.mount(primary, {
        message: 'Could not load account: ' + ((e && e.message) || 'unknown'),
        onRetry: loadAccount,
      });
      return;
    }
    currentAccount = info || {};
    const handleEl = document.getElementById('ad-handle');
    const metaEl = document.getElementById('ad-meta');
    const bcEl = document.getElementById('ad-handle-bc');
    const handle = info.handle || 'unknown';
    if (handleEl) {
      handleEl.innerHTML = '@' + esc(handle) + ' ' +
        (global.AuroraStatusBadge
          ? global.AuroraStatusBadge.render(info.status || 'active', info.status || 'Active')
          : '');
    }
    if (bcEl) bcEl.textContent = '@' + handle;
    if (metaEl) {
      const created = global.AuroraTimestamp.render({ value: info.createdAt, context: 'detail' });
      metaEl.innerHTML = '<code>' + esc(currentDid) + '</code>' +
                        (created ? ' · Member since ' + created : '');
    }
    renderDrawers(info);
  }

  function renderDrawers(info) {
    const session = global.AuroraSession;
    const isAdmin = session && session.hasRole('admin');
    const isMod = session && session.hasRole('moderator');
    const isSuper = session && session.hasRole('superadmin');
    const drawer = global.AuroraDrawer;
    const primary = document.getElementById('ad-primary');
    if (!primary || !drawer) return;
    let html = '';
    // Always-visible: Account overview
    html += drawer.render({
      id: 'overview-' + currentDid,
      summary: '<strong>Account overview</strong>',
      open: true,
      bodyHtml: overviewHtml(info),
    });
    // Moderation actions (Mod+)
    if (isMod) {
      html += drawer.render({
        id: 'mod-actions-' + currentDid,
        summary: '<strong>Moderation actions</strong>',
        roleTag: 'Moderator+',
        open: true,
        bodyHtml: '<div id="ad-mod-action-panel"></div>',
      });
    }
    // Account management (Admin+)
    if (isAdmin) {
      html += drawer.render({
        id: 'mgmt-' + currentDid,
        summary: '<strong>Account management</strong>',
        roleTag: 'Admin+',
        bodyHtml: managementHtml(info),
      });
    }
    // Subject history (Mod+)
    if (isMod) {
      html += drawer.render({
        id: 'history-' + currentDid,
        summary: '<strong>Subject history</strong>',
        roleTag: 'Moderator+',
        bodyHtml: '<div id="ad-history-body">' + global.AuroraSkeleton.lines(3) + '</div>',
      });
    }
    // Kryphocron audience visibility (Mod+, §6.5) — read-only.
    if (isMod) {
      html += drawer.render({
        id: 'kryphocron-' + currentDid,
        summary: '<strong>' + esc(T('kryphocron.drawer.title')) + '</strong>',
        roleTag: 'Moderator+',
        bodyHtml: '<div id="ad-kryphocron-body">' + global.AuroraSkeleton.lines(3) + '</div>',
      });
    }
    // Kryphocron overrides (SuperAdmin, §6.6.2 item 4 / #316) — per-account
    // policy exceptions, distinct from the Mod+ read-only audience drawer above.
    if (isSuper) {
      html += drawer.render({
        id: 'kryphocron-overrides-' + currentDid,
        summary: '<strong>' + esc(T('kryphocron.overrides.title')) + '</strong>',
        roleTag: 'SuperAdmin',
        bodyHtml: '<div id="ad-overrides-body">' + global.AuroraSkeleton.lines(3) + '</div>',
      });
    }
    primary.innerHTML = html;
    drawer.attach(primary);

    // Kryphocron drawer content (audiences owned + block-cascade impact),
    // loaded async from the #225 per-account read endpoints.
    if (isMod) loadKryphocronDrawer(currentDid);
    // Per-account override controls (SuperAdmin, #316).
    if (isSuper) loadOverridesDrawer(currentDid);

    // Mount Pattern B (moderation) ActionPanel.
    const modPanelHost = document.getElementById('ad-mod-action-panel');
    if (modPanelHost && typeof ActionPanel === 'function') {
      const panel = new ActionPanel({
        subject: { '$type': 'com.atproto.admin.defs#repoRef', did: currentDid },
        availableActions: ['TakedownAccount', 'SuspendAccount', 'RestoreAccount', 'ApplyLabel', 'RemoveLabel', 'SendEmail'],
        defaultAction: 'TakedownAccount',
        requiresRationale: true,
        highImpactActions: ['TakedownAccount'],
        userRole: session ? session.role() : 'moderator',
        onCancel: () => { /* drawer stays open */ },
      });
      panel.mount(modPanelHost);
    }
    wireManagementHandlers();
  }

  // Kryphocron audience-visibility drawer (§6.5): audiences owned (read-only,
  // never the members contents) + block-cascade impact. Default-audience and
  // per-account cadence are omitted (not host-exposed yet — §6.5 "surface if
  // exposed; otherwise omit"). Reads the #225 per-account endpoints.
  async function loadKryphocronDrawer(did) {
    const K = global.AuroraEndpoints && global.AuroraEndpoints.ops && global.AuroraEndpoints.ops.kryphocron;
    const host = document.getElementById('ad-kryphocron-body');
    if (!host || !K) return;
    const [audRes, casRes] = await Promise.all([
      K.listAudiences(did).catch((e) => ({ __err: e })),
      K.getBlockCascadeImpact(did).catch((e) => ({ __err: e })),
    ]);
    // Bail if the operator navigated to another account meanwhile.
    if (currentDid !== did) return;

    let html = '<h4 class="drawer-subhead">' + esc(T('kryphocron.drawer.audiences_title')) + '</h4>';
    if (audRes && audRes.__err) {
      html += '<p class="empty-state">' + esc(T('kryphocron.drawer.audiences_error')) + '</p>';
    } else {
      const audiences = (audRes && audRes.audiences) || [];
      if (!audiences.length) {
        html += '<p class="empty-state">' + esc(T('kryphocron.drawer.audiences_empty')) + '</p>';
      } else {
        html += '<ul class="ad-audience-list">' + audiences.map(function (a) {
          const mode = a.mode ? T('kryphocron.audiences.mode_' + a.mode) : T('kryphocron.audiences.mode_unset');
          const members = (a.mode === 'list' && a.memberCount != null)
            ? ' · ' + esc(T('kryphocron.drawer.members', { count: a.memberCount }))
            : '';
          return '<li><strong>' + esc(a.name || a.rkey || '—') + '</strong> ' +
            '<span class="badge">' + esc(mode) + '</span>' + members + '</li>';
        }).join('') + '</ul>';
      }
    }

    html += '<h4 class="drawer-subhead">' + esc(T('kryphocron.drawer.cascade_title')) + '</h4>';
    if (casRes && casRes.__err) {
      html += '<p class="empty-state">' + esc(T('kryphocron.drawer.cascade_error')) + '</p>';
    } else if (casRes && casRes.available) {
      html += '<p>' + esc(T('kryphocron.drawer.cascade_count', { count: casRes.cascadeRemovals || 0 })) + '</p>';
    } else {
      html += '<p class="empty-state">' + esc(T('kryphocron.drawer.cascade_pending')) + '</p>';
    }

    host.innerHTML = html;
  }

  // Per-account override controls (SuperAdmin, §6.6.2 item 4 / #316): two
  // checkboxes (block capability-issuance; exempt from rate limits — stored,
  // enforced when per-tier limits ship) + a required rationale + an audit pivot.
  async function loadOverridesDrawer(did) {
    const K = global.AuroraEndpoints && global.AuroraEndpoints.ops && global.AuroraEndpoints.ops.kryphocron;
    const host = document.getElementById('ad-overrides-body');
    if (!host || !K) return;
    let ov = {};
    try {
      const res = await K.getAccountOverrides(did);
      ov = (res && res.overrides) || {};
    } catch (e) {
      host.innerHTML = '<p class="empty-state">' + esc(T('kryphocron.overrides.error')) + '</p>';
      return;
    }
    if (currentDid !== did) return;
    const blocked = ov.capabilityIssuance === false;
    const rlExempt = ov.rateLimitExempt === true;
    host.innerHTML =
      '<label class="ad-ov-row"><input type="checkbox" id="ad-ov-block"' + (blocked ? ' checked' : '') + '> ' +
        esc(T('kryphocron.overrides.block_label')) + '</label>' +
      '<label class="ad-ov-row"><input type="checkbox" id="ad-ov-ratelimit"' + (rlExempt ? ' checked' : '') + '> ' +
        esc(T('kryphocron.overrides.ratelimit_label')) + '</label>' +
      '<p class="settings-help">' + esc(T('kryphocron.overrides.ratelimit_note')) + '</p>' +
      '<label class="ad-ov-rationale">' + esc(T('kryphocron.overrides.rationale_label')) +
        '<textarea id="ad-ov-rationale" rows="2"></textarea></label>' +
      '<div class="ad-ov-actions">' +
        '<button type="button" class="btn-primary btn-sm" id="ad-ov-save">' + esc(T('common.save')) + '</button>' +
        '<a class="btn-secondary btn-sm" href="#mod/audit?subject=' + encodeURIComponent(did) + '">' +
          esc(T('kryphocron.overrides.audit_pivot')) + '</a>' +
      '</div>';
    const saveBtn = document.getElementById('ad-ov-save');
    if (saveBtn) saveBtn.addEventListener('click', function () { saveOverride(did); });
  }

  async function saveOverride(did) {
    const K = global.AuroraEndpoints.ops.kryphocron;
    const rationale = (document.getElementById('ad-ov-rationale').value || '').trim();
    if (!rationale) { global.AuroraToast.warning(T('kryphocron.overrides.rationale_required')); return; }
    // Checkbox → full-state value: blocked sets capabilityIssuance=false,
    // unchecked clears to null (default allowed); exempt sets rateLimitExempt=true.
    const body = {
      did: did,
      capabilityIssuance: document.getElementById('ad-ov-block').checked ? false : null,
      rateLimitExempt: document.getElementById('ad-ov-ratelimit').checked ? true : null,
      rationale: rationale,
    };
    try {
      const res = await K.setAccountOverride(body);
      global.AuroraToast.success(T('kryphocron.overrides.saved'), res && res.auditEntryId ? {
        action: { label: T('settings.roles.view_audit'), href: '#mod/audit/' + encodeURIComponent(res.auditEntryId) },
      } : undefined);
      loadOverridesDrawer(did);
    } catch (e) {
      global.AuroraToast.danger(T('kryphocron.overrides.save_failed') + (e && e.message ? ': ' + e.message : ''));
    }
  }

  function overviewHtml(info) {
    const fmt = global.AuroraFormat;
    return '<dl class="ad-overview">' +
           defItem('Handle', '@' + esc(info.handle || '—')) +
           defItem('DID', '<code>' + esc(currentDid) + '</code>') +
           defItem('Email', esc(info.email || 'N/A')) +
           defItem('Created', global.AuroraTimestamp.render({ value: info.createdAt, context: 'detail' })) +
           defItem('Posts', String(info.postsCount || 0)) +
           defItem('Followers', String(info.followersCount || 0)) +
           defItem('Following', String(info.followingCount || 0)) +
           '</dl>';
  }

  function defItem(label, value) {
    return '<div style="display:flex; justify-content:space-between; padding: 0.25rem 0; border-bottom: 1px solid var(--color-border-primary);">' +
           '<dt style="color: var(--color-text-secondary);">' + esc(label) + '</dt>' +
           '<dd>' + value + '</dd></div>';
  }

  function managementHtml(info) {
    return '<div class="ad-mgmt-sections">' +
           '  <fieldset>' +
           '    <legend>Identity</legend>' +
           '    <div class="form-group">' +
           '      <label>Email</label>' +
           '      <input type="email" id="ad-mgmt-email" value="' + esc(info.email || '') + '">' +
           '      <button type="button" class="btn-sm btn-primary" data-action="set-email">Update email</button>' +
           '    </div>' +
           '    <div class="form-group">' +
           '      <label>Handle</label>' +
           '      <input type="text" id="ad-mgmt-handle" value="' + esc(info.handle || '') + '">' +
           '      <button type="button" class="btn-sm btn-primary" data-action="set-handle">Update handle</button>' +
           '    </div>' +
           '  </fieldset>' +
           '  <fieldset>' +
           '    <legend>Credentials</legend>' +
           '    <div class="action-panel-buttons" style="justify-content: flex-start; gap: 0.5rem; flex-wrap: wrap;">' +
           '      <button type="button" class="btn-secondary" data-action="send-reset">Send password reset</button>' +
           '      <button type="button" class="btn-danger" data-action="override-password">Override password</button>' +
           '      <button type="button" class="btn-secondary" data-action="signing-key">Update signing key</button>' +
           '    </div>' +
           '  </fieldset>' +
           '  <fieldset>' +
           '    <legend>Lifecycle</legend>' +
           '    <div class="action-panel-buttons" style="justify-content: flex-start; gap: 0.5rem; flex-wrap: wrap;">' +
           '      <button type="button" class="btn-secondary" data-action="invites-toggle">Toggle account invites</button>' +
           '      <button type="button" class="btn-danger" data-action="delete-account">Delete account</button>' +
           '      <button type="button" class="btn-secondary" data-action="forensic">Generate forensic export</button>' +
           '    </div>' +
           '  </fieldset>' +
           '</div>';
  }

  function wireManagementHandlers() {
    const root = document.getElementById('ad-primary');
    if (!root) return;
    root.querySelectorAll('[data-action]').forEach((btn) => {
      btn.addEventListener('click', () => handleManagementAction(btn.dataset.action));
    });
  }

  async function handleManagementAction(action) {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    if (action === 'send-reset') return doPasswordReset();
    if (action === 'override-password') return overridePassword();
    if (action === 'signing-key') return updateSigningKey();
    if (action === 'set-email') return updateEmail();
    if (action === 'set-handle') return updateHandle();
    if (action === 'invites-toggle') return toggleInvites();
    if (action === 'delete-account') return deleteAccount();
    if (action === 'forensic') return openForensicExport();
  }

  async function doPasswordReset() {
    const rationale = await promptRationale('Send password reset email?',
      'Subject: ' + (currentAccount.handle || currentDid), 'Send reset email');
    if (rationale == null) return;
    try {
      const res = await global.AuroraCapabilities.callEndpoint('trigger-password-reset', {
        did: currentDid, rationale: rationale,
      });
      const sent = res.resetEmailSent
        ? 'Password reset email sent to ' + (res.maskedEmail || '')
        : 'Token generated; email not sent (mailer not configured).';
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success(sent, auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
    } catch (e) {
      global.AuroraToast.danger('Password reset failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function overridePassword() {
    const result = await global.AuroraModal.form({
      heading: 'Override password',
      body: 'Sets a new password directly on the account. The operator must communicate the new credential to the account holder out-of-band.',
      fields: [
        { name: 'newPwd', label: 'New password', type: 'password', required: true },
        { name: 'rationale', label: 'Rationale (recorded in audit log)', type: 'textarea', required: true },
      ],
      submitLabel: 'Override password',
    });
    if (!result.submitted) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.updateAccountPassword', {
        did: currentDid, password: result.values.newPwd,
      });
      global.AuroraToast.success('Password override applied.');
    } catch (e) {
      global.AuroraToast.danger('Override failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function updateSigningKey() {
    const result = await global.AuroraModal.form({
      heading: 'Update signing key',
      body: 'Updates the account\'s signing key. Affects identity verification across federation.',
      fields: [
        { name: 'didKey', label: 'New signing key (DID-key form)', type: 'text', required: true },
        { name: 'rationale', label: 'Rationale (recorded in audit log)', type: 'textarea', required: true },
      ],
      submitLabel: 'Update signing key',
    });
    if (!result.submitted) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.updateAccountSigningKey', {
        did: currentDid, signingKey: result.values.didKey,
      });
      global.AuroraToast.success('Signing key updated.');
    } catch (e) {
      global.AuroraToast.danger('Update failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function updateEmail() {
    const newEmail = document.getElementById('ad-mgmt-email').value.trim();
    if (!newEmail) return global.AuroraToast.warning('Email cannot be empty.');
    const rationale = await promptRationale('Update email to ' + newEmail + '?', null, 'Update email');
    if (rationale == null) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.updateAccountEmail', {
        did: currentDid, email: newEmail,
      });
      global.AuroraToast.success('Email updated.');
      currentAccount.email = newEmail;
    } catch (e) {
      global.AuroraToast.danger('Update failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function updateHandle() {
    const newHandle = document.getElementById('ad-mgmt-handle').value.trim();
    if (!newHandle) return global.AuroraToast.warning('Handle cannot be empty.');
    const rationale = await promptRationale('Update handle to @' + newHandle + '?', null, 'Update handle');
    if (rationale == null) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.updateAccountHandle', {
        did: currentDid, handle: newHandle,
      });
      global.AuroraToast.success('Handle updated.');
      currentAccount.handle = newHandle;
    } catch (e) {
      global.AuroraToast.danger('Update failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function toggleInvites() {
    // The prior native confirm() inverted OK/Cancel (OK to disable,
    // Cancel to enable) — cognitive load operators routinely
    // misread. The modal makes the binary explicit via a select
    // dropdown; the operator picks the target state.
    const result = await global.AuroraModal.form({
      heading: 'Toggle account invites',
      body: 'Set the invite state for this account.',
      fields: [
        {
          name: 'state',
          label: 'New state',
          type: 'select',
          options: [
            { value: 'disabled', label: 'Disabled' },
            { value: 'enabled',  label: 'Enabled' },
          ],
          default: 'disabled',
          required: true,
        },
        {
          name: 'rationale',
          label: 'Rationale (recorded in audit log)',
          type: 'textarea',
          required: true,
        },
      ],
      submitLabel: 'Save state',
    });
    if (!result.submitted) return;
    const enable = result.values.state === 'enabled';
    try {
      const ep = enable
        ? 'com.atproto.admin.enableAccountInvites'
        : 'com.atproto.admin.disableAccountInvites';
      await global.AuroraClient.post(ep, { account: currentDid, note: result.values.rationale });
      global.AuroraToast.success((enable ? 'Enabled' : 'Disabled') + ' invites.');
    } catch (e) {
      global.AuroraToast.danger('Toggle failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function deleteAccount() {
    const handle = currentAccount.handle || '';
    const result = await global.AuroraModal.destructiveConfirm({
      heading: 'Delete account',
      body: 'This permanently removes the account, its records, and its invite lineage. Irreversible.',
      typedConfirmGate: handle,
      rationaleRequired: true,
      ackCheckbox: 'I understand this is irreversible',
      confirmLabel: 'Delete account',
    });
    if (!result.confirmed) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.deleteAccount', {
        did: currentDid,
      });
      global.AuroraToast.success('Account deleted.');
      if (global.AuroraRouter) global.AuroraRouter.navigate('ops/accounts');
    } catch (e) {
      global.AuroraToast.danger('Delete failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function openForensicExport() {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    const div = document.createElement('div');
    div.innerHTML = forensicExportBody(isSuper);
    const handle = global.AuroraModal.open({
      title: 'Generate forensic export',
      body: div,
    });
    div.querySelector('#fx-cancel').addEventListener('click', () => handle.close());
    div.querySelector('#fx-submit').addEventListener('click', () => submitForensicExport(handle));
  }

  function forensicExportBody(isSuper) {
    return '<p><strong>Subject:</strong> ' +
           esc((currentAccount.handle ? '@' + currentAccount.handle + ' — ' : '') + currentDid) + '</p>' +
           '<fieldset>' +
           '  <legend>Include</legend>' +
           '  <label style="display:block;"><input type="checkbox" id="fx-repo" checked> Repository content (CAR file) — deferred to v0.3</label>' +
           '  <label style="display:block;"><input type="checkbox" id="fx-blobs" checked> Blobs — deferred to v0.3</label>' +
           '  <label style="display:block;"><input type="checkbox" id="fx-mod" checked> Moderation history</label>' +
           '  <label style="display:block;"><input type="checkbox" id="fx-meta"' + (isSuper ? '' : ' disabled') +
           '> Account metadata <span class="role-tag">SuperAdmin only</span></label>' +
           '  <label style="display:block;"><input type="checkbox" id="fx-audit"' + (isSuper ? '' : ' disabled') +
           '> Audit chain entries <span class="role-tag">SuperAdmin only</span></label>' +
           '</fieldset>' +
           '<label style="display:block; margin-top: 0.5rem;">Rationale (required)</label>' +
           '<textarea id="fx-rationale" rows="3" style="width:100%;" aria-required="true"></textarea>' +
           '<p class="action-panel-hint" style="margin-top: 0.5rem;">' +
           '  This export will be recorded in the audit chain with a tamper-evident hash. ' +
           '  The bundle will contain account data; treat as sensitive.' +
           '</p>' +
           '<div class="action-panel-buttons" style="margin-top: 0.75rem;">' +
           '  <button class="btn-secondary" id="fx-cancel">Cancel</button>' +
           '  <button class="btn-danger" id="fx-submit">Generate export</button>' +
           '</div>';
  }

  async function submitForensicExport(modalHandle) {
    const rationale = document.getElementById('fx-rationale').value.trim();
    if (!rationale) return global.AuroraToast.warning('Rationale is required.');
    const body = {
      did: currentDid,
      rationale: rationale,
      includeRepo: document.getElementById('fx-repo').checked,
      includeBlobs: document.getElementById('fx-blobs').checked,
      includeModerationHistory: document.getElementById('fx-mod').checked,
      includeAccountMetadata: document.getElementById('fx-meta').checked,
      includeAuditChain: document.getElementById('fx-audit').checked,
    };
    try {
      const res = await global.AuroraEndpoints.admin.exportAccountForensicRaw(body);
      if (!res.ok) {
        let detail = '';
        try { const j = await res.json(); detail = ': ' + (j.message || j.error || ''); } catch (e) {}
        throw new Error('HTTP ' + res.status + detail);
      }
      const auditId = res.headers.get('X-Aurora-Audit-Entry-Id');
      const bundleHash = res.headers.get('X-Aurora-Bundle-Hash');
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = 'forensic-export-' + currentDid.replace(/:/g, '_') + '-' +
                   new Date().toISOString().replace(/[:.]/g, '') + '.tar';
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      modalHandle.close();
      // Bundle hash stays inline (it's the verification token operators
      // need to keep with the bundle); the audit-entry id moves into a
      // click-through action link per Arc 6 Step 3 sub-3e.
      const msg = 'Export complete. Bundle hash: ' + bundleHash;
      global.AuroraToast.success(msg, auditId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditId),
        },
      } : undefined);
    } catch (e) {
      global.AuroraToast.danger('Export failed: ' + (e && e.message ? e.message : ''));
    }
  }

  // Inline rationale prompt — uses a modal to ensure proper a11y. The
  // confirm button takes a descriptive, action-naming label (§8.2.3 — not a
  // generic "Confirm"); the caller passes the verb that matches `title`.
  function promptRationale(title, subtext, confirmLabel) {
    return new Promise((resolve) => {
      const div = document.createElement('div');
      div.innerHTML = (subtext ? '<p>' + esc(subtext) + '</p>' : '') +
                      '<label>Rationale (required)</label>' +
                      '<textarea id="pr-r" rows="3" style="width:100%;"></textarea>' +
                      '<div class="action-panel-buttons" style="margin-top: 0.75rem;">' +
                      '  <button class="btn-secondary" id="pr-cancel">Cancel</button>' +
                      '  <button class="btn-primary" id="pr-confirm">' +
                        esc(confirmLabel || 'Submit') + '</button>' +
                      '</div>';
      const handle = global.AuroraModal.open({ title: title, body: div, onClose: () => resolve(null) });
      div.querySelector('#pr-cancel').addEventListener('click', () => { handle.close(); });
      div.querySelector('#pr-confirm').addEventListener('click', () => {
        const v = div.querySelector('#pr-r').value.trim();
        if (!v) { global.AuroraToast.warning('Rationale is required.'); return; }
        handle.close();
        // onClose resolves null; trigger a separate resolve here.
        // Replace the resolve function so onClose is a noop.
        resolve(v);
      });
    });
  }


  // ---------- Rail panels ----------

  async function loadSubjectContext() {
    const rail = document.getElementById('ad-rail');
    if (!rail) return;
    rail.insertAdjacentHTML('beforeend', railCard('subject-context', 'Subject context',
      global.AuroraSkeleton.lines(3)));
    rail.insertAdjacentHTML('beforeend', railCard('records-authored', 'Records authored',
      global.AuroraSkeleton.lines(3)));
    rail.insertAdjacentHTML('beforeend', railCard('blob-inventory', 'Blob inventory',
      global.AuroraSkeleton.lines(3)));
    rail.insertAdjacentHTML('beforeend', railCard('invite-lineage', 'Invite lineage',
      global.AuroraSkeleton.lines(3)));
    rail.insertAdjacentHTML('beforeend', railCard('account-roles', T('accountDetail.roles.title'),
      global.AuroraSkeleton.lines(3)));

    try {
      const ctx = await global.AuroraEndpoints.moderator.getSubjectContext({ did: currentDid });
      renderSubjectContext(ctx);
    } catch (e) {
      const body = railBodyEl('subject-context');
      if (body) global.AuroraInlineError.mount(body, {
        message: 'Could not load context: ' + ((e && e.message) || ''),
        onRetry: loadSubjectContext,
      });
    }
  }

  function renderSubjectContext(ctx) {
    const recentReports = (ctx && ctx.recentReports) || [];
    const recentEvents = (ctx && ctx.recentActions) || [];
    const recentAppeals = (ctx && ctx.recentAppeals) || [];
    const labels = (ctx && ctx.externalLabels) || [];
    let html = '';
    html += sectionTitle('Recent reports') + listOrEmpty(recentReports.slice(0, 5).map((r) =>
      '<li>' + (global.AuroraEntityRef ? global.AuroraEntityRef.report ? global.AuroraEntityRef.report(r.id) : '#' + esc(r.id) : '#' + esc(r.id)) +
      ' — ' + esc(r.reasonType || '') + '</li>'));
    html += sectionTitle('Recent actions') + listOrEmpty(recentEvents.slice(0, 5).map((e) =>
      '<li>' + (global.AuroraEntityRef ? global.AuroraEntityRef.event(e.id) : '#' + esc(e.id)) +
      ' — ' + esc(e.eventType || e.action || '') + '</li>'));
    html += sectionTitle('Recent appeals') + listOrEmpty(recentAppeals.slice(0, 5).map((a) =>
      '<li>' + (global.AuroraEntityRef ? global.AuroraEntityRef.appeal(a.id) : '#' + esc(a.id)) +
      ' — ' + esc(a.status || '') + '</li>'));
    html += sectionTitle('External labels') + listOrEmpty(labels.slice(0, 10).map((l) =>
      '<li>' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render('pending', l.val || l) : esc(l.val || l)) + '</li>'));
    setRailBody('subject-context', html);
  }

  async function loadSubjectHistory() {
    const host = document.getElementById('ad-history-body');
    if (!host) return;
    try {
      const data = await global.AuroraEndpoints.moderator.getSubjectHistory({ did: currentDid, limit: 25 });
      const items = (data && data.items) || [];
      if (items.length === 0) {
        host.innerHTML = '<p class="empty-state">No prior actions on this account.</p>';
        return;
      }
      const fmt = global.AuroraFormat;
      host.innerHTML = '<ul style="list-style:none; padding:0;">' + items.map((it) =>
        '<li style="padding: 0.5rem 0; border-bottom: 1px solid var(--color-border-primary);">' +
        '<strong>' + esc(it.eventType || it.action || '') + '</strong>' +
        ' by ' + (global.AuroraEntityRef ? global.AuroraEntityRef.account(it.actorDid) : esc(it.actorDid)) +
        ' — ' + global.AuroraTimestamp.render({ value: it.createdAt || it.timestamp, context: 'activity' }) +
        (it.id != null ? ' · ' + (global.AuroraEntityRef ? global.AuroraEntityRef.event(it.id) : '#' + esc(it.id)) : '') +
        '</li>').join('') + '</ul>';
    } catch (e) {
      global.AuroraInlineError.mount(host, {
        message: 'Could not load history: ' + ((e && e.message) || ''),
        onRetry: loadSubjectHistory,
      });
    }
  }

  async function loadRecords() {
    try {
      const repo = currentDid;
      const data = await global.AuroraClient.get('com.atproto.repo.listRecords', {
        repo: repo, collection: 'app.bsky.feed.post', limit: 5,
      });
      const records = (data && data.records) || [];
      if (records.length === 0) {
        setRailBody('records-authored', '<p class="empty-state">No records authored.</p>');
        return;
      }
      setRailBody('records-authored',
        '<ul style="list-style:none; padding:0;">' + records.map((r) =>
          '<li style="padding: 0.25rem 0;">' +
          (global.AuroraEntityRef ? global.AuroraEntityRef.record(r.uri) : '<code>' + esc(r.uri) + '</code>') +
          '</li>').join('') + '</ul>');
    } catch (e) {
      const body = railBodyEl('records-authored');
      if (body) global.AuroraInlineError.mount(body, {
        message: 'Could not load records: ' + ((e && e.message) || ''),
        onRetry: loadRecords,
      });
    }
  }

  async function loadBlobs() {
    try {
      const data = await global.AuroraEndpoints.ops.listBlobs({ did: currentDid, limit: 5 });
      const blobs = (data && data.blobs) || [];
      if (blobs.length === 0) {
        setRailBody('blob-inventory', '<p class="empty-state">No owned blobs.</p>');
        return;
      }
      setRailBody('blob-inventory',
        '<ul style="list-style:none; padding:0;">' + blobs.map((b) =>
          '<li style="padding: 0.25rem 0;">' +
          (global.AuroraEntityRef ? global.AuroraEntityRef.blob(b.cid) : '<code>' + esc(b.cid) + '</code>') +
          (b.size ? ' <span style="color: var(--color-text-tertiary);">' +
                    (global.AuroraFormat ? global.AuroraFormat.bytes(b.size) : '') + '</span>' : '') +
          '</li>').join('') + '</ul>');
    } catch (e) {
      const body = railBodyEl('blob-inventory');
      if (body) global.AuroraInlineError.mount(body, {
        message: 'Could not load blobs: ' + ((e && e.message) || ''),
        onRetry: loadBlobs,
      });
    }
  }

  async function loadInvites() {
    try {
      const data = await global.AuroraClient.get('com.atproto.admin.getInviteCodes', { did: currentDid, limit: 10 });
      const codes = (data && (data.codes || data.inviteCodes)) || [];
      if (codes.length === 0) {
        setRailBody('invite-lineage', '<p class="empty-state">No invite codes for this account.</p>');
        return;
      }
      setRailBody('invite-lineage',
        '<ul style="list-style:none; padding:0;">' + codes.slice(0, 10).map((c) =>
          '<li style="padding: 0.25rem 0;">' +
          (global.AuroraEntityRef ? global.AuroraEntityRef.invite(c.code) : '<code>' + esc(c.code) + '</code>') +
          ' <span style="color: var(--color-text-tertiary);">(' + (c.uses || 0) + '/' + (c.available || 1) + ')</span>' +
          '</li>').join('') + '</ul>');
    } catch (e) {
      const body = railBodyEl('invite-lineage');
      if (body) global.AuroraInlineError.mount(body, {
        message: 'Could not load invites: ' + ((e && e.message) || ''),
        onRetry: loadInvites,
      });
    }
  }

  // ---------- Rail helpers ----------

  // §10.2.4 Roles panel: surface this account's admin role (if any) in the
  // rail with a navigation-only pivot to Configuration → Roles. list_roles
  // with ?did returns { did, role } where role is the role string (or the
  // AdminRole object, or null for a non-operator account). Rationale-wiring
  // of management actions stays Arc F; this is read + navigate only.
  async function loadRoles() {
    try {
      const data = await global.AuroraEndpoints.atproto.listRoles({ did: currentDid });
      const roleRec = data && data.role;
      const roleName = (typeof roleRec === 'string') ? roleRec : (roleRec && roleRec.role);
      if (!roleName) {
        setRailBody('account-roles',
          '<p class="empty-state">' + esc(T('accountDetail.roles.none')) + '</p>' +
          '<p><a href="#configuration/roles">' + esc(T('accountDetail.roles.manage')) + '</a></p>');
        return;
      }
      setRailBody('account-roles',
        '<p>' + esc(T('accountDetail.roles.role_label', { role: roleName })) + '</p>' +
        '<p><a href="#configuration/roles">' + esc(T('accountDetail.roles.open_mgmt')) + '</a></p>');
    } catch (e) {
      const body = railBodyEl('account-roles');
      if (body) global.AuroraInlineError.mount(body, {
        message: T('accountDetail.roles.error'),
        onRetry: loadRoles,
      });
    }
  }

  function railCard(id, title, body) {
    return '<div class="rail-card" data-rail-id="' + id + '">' +
           '  <h4>' + esc(title) + '</h4>' +
           '  <div data-rail-body>' + body + '</div>' +
           '</div>';
  }

  function setRailBody(id, html) {
    const body = railBodyEl(id);
    if (body) body.innerHTML = html;
  }

  // Resolve the inner body element of a rail card so error primitives can
  // mount() onto it (mount() takes a DOM element, not an HTML string).
  function railBodyEl(id) {
    const card = document.querySelector('[data-rail-id="' + id + '"]');
    if (!card) return null;
    return card.querySelector('[data-rail-body]');
  }

  function sectionTitle(t) {
    return '<h5 style="margin: 0.75rem 0 0.25rem 0; font-size: 0.75rem; ' +
           'text-transform: uppercase; letter-spacing: 0.06em; color: var(--color-text-tertiary);">' +
           esc(t) + '</h5>';
  }

  function listOrEmpty(items) {
    if (!items || items.length === 0) return '<p style="color: var(--color-text-tertiary); font-size: 0.8125rem;">None</p>';
    return '<ul style="list-style:none; padding:0; font-size: 0.875rem;">' + items.join('') + '</ul>';
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }

  if (global.AuroraRouter) global.AuroraRouter.register('opsAccountDetail', { mount: mount });
})(window);
