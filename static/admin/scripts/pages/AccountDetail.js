// Account detail page (route: #ops/accounts/:did).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.2 — the canonical per-account
// surface. Two-column layout: drawers (Account overview, Moderation
// actions, Account management, Subject history) on the left, context
// rail (Subject context, Records authored, Blob inventory, Invite
// lineage) on the right. Drawer-level role gating per §5.2.4.

(function (global) {
  'use strict';

  let currentDid = null;
  let currentAccount = null;
  let currentRoleCheck = null;

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
    ]);
    return { unmount: () => { currentDid = null; currentAccount = null; } };
  }

  function renderShell(did) {
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
      if (primary) primary.innerHTML = '<p class="empty-state">Could not load account: ' +
                                       (e && e.message ? esc(e.message) : 'unknown') + '</p>';
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
      const created = global.AuroraFormat ? global.AuroraFormat.date(info.createdAt, 'medium') : '';
      metaEl.innerHTML = '<code>' + esc(currentDid) + '</code>' +
                        (created ? ' · Member since ' + esc(created) : '');
    }
    renderDrawers(info);
  }

  function renderDrawers(info) {
    const session = global.AuroraSession;
    const isAdmin = session && session.hasRole('admin');
    const isMod = session && session.hasRole('moderator');
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
        bodyHtml: '<div id="ad-history-body"><p class="empty-state">Loading…</p></div>',
      });
    }
    primary.innerHTML = html;
    drawer.attach(primary);

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
    // Pivot links wiring (Roles panel — render to rail when data available)
  }

  function overviewHtml(info) {
    const fmt = global.AuroraFormat;
    return '<dl class="ad-overview">' +
           defItem('Handle', '@' + esc(info.handle || '—')) +
           defItem('DID', '<code>' + esc(currentDid) + '</code>') +
           defItem('Email', esc(info.email || 'N/A')) +
           defItem('Created', fmt ? esc(fmt.date(info.createdAt, 'medium')) : esc(info.createdAt || '')) +
           defItem('Posts', String(info.postsCount || 0)) +
           defItem('Followers', String(info.followersCount || 0)) +
           defItem('Following', String(info.followingCount || 0)) +
           '</dl>';
  }

  function defItem(label, value) {
    return '<div style="display:flex; justify-content:space-between; padding: 0.25rem 0; border-bottom: 1px solid var(--border-color);">' +
           '<dt style="color: var(--text-secondary);">' + esc(label) + '</dt>' +
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
      'Subject: ' + (currentAccount.handle || currentDid));
    if (rationale == null) return;
    try {
      const res = await global.AuroraCapabilities.callEndpoint('trigger-password-reset', {
        did: currentDid, rationale: rationale,
      });
      const sent = res.resetEmailSent
        ? 'Password reset email sent to ' + (res.maskedEmail || '')
        : 'Token generated; email not sent (mailer not configured).';
      global.AuroraToast.success(sent);
    } catch (e) {
      global.AuroraToast.danger('Password reset failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function overridePassword() {
    const rationale = await promptRationaleAndConfirmation(
      'Override password',
      'This is irreversible. The operator must communicate the new credential to the account holder out-of-band.',
      'I understand this overrides without notifying the account holder',
    );
    if (!rationale) return;
    const newPwd = prompt('New password:');
    if (!newPwd) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.updateAccountPassword', {
        did: currentDid, password: newPwd,
      });
      global.AuroraToast.success('Password override applied.');
    } catch (e) {
      global.AuroraToast.danger('Override failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function updateSigningKey() {
    const rationale = await promptRationaleAndConfirmation(
      'Update signing key',
      'Updating the signing key affects identity verification across federation. This is irreversible.',
      'I understand this is irreversible',
    );
    if (!rationale) return;
    const key = prompt('New signing key (DID-key form):');
    if (!key) return;
    try {
      await global.AuroraClient.post('com.atproto.admin.updateAccountSigningKey', {
        did: currentDid, signingKey: key,
      });
      global.AuroraToast.success('Signing key updated.');
    } catch (e) {
      global.AuroraToast.danger('Update failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function updateEmail() {
    const newEmail = document.getElementById('ad-mgmt-email').value.trim();
    if (!newEmail) return global.AuroraToast.warning('Email cannot be empty.');
    const rationale = await promptRationale('Update email to ' + newEmail + '?');
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
    const rationale = await promptRationale('Update handle to @' + newHandle + '?');
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
    const enable = !confirm('Press OK to disable account invites, Cancel to enable.');
    const rationale = await promptRationale((enable ? 'Enable' : 'Disable') + ' account invites?');
    if (rationale == null) return;
    try {
      const ep = enable
        ? 'com.atproto.admin.enableAccountInvites'
        : 'com.atproto.admin.disableAccountInvites';
      await global.AuroraClient.post(ep, { account: currentDid, note: rationale });
      global.AuroraToast.success((enable ? 'Enabled' : 'Disabled') + ' invites.');
    } catch (e) {
      global.AuroraToast.danger('Toggle failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function deleteAccount() {
    const handle = currentAccount.handle || '';
    const typed = prompt('Type the account handle (' + handle + ') to confirm deletion:');
    if (typed !== handle) {
      if (typed != null) global.AuroraToast.warning('Handle did not match; cancelled.');
      return;
    }
    const rationale = await promptRationaleAndConfirmation(
      'Delete account',
      'This permanently removes the account, its records, and its invite lineage. Irreversible.',
      'I understand this is irreversible',
    );
    if (!rationale) return;
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
      global.AuroraToast.success('Export complete. Audit entry: ' + auditId + ', bundle hash: ' + bundleHash);
    } catch (e) {
      global.AuroraToast.danger('Export failed: ' + (e && e.message ? e.message : ''));
    }
  }

  // Inline rationale prompt — uses a modal to ensure proper a11y.
  function promptRationale(title, subtext) {
    return new Promise((resolve) => {
      const div = document.createElement('div');
      div.innerHTML = (subtext ? '<p>' + esc(subtext) + '</p>' : '') +
                      '<label>Rationale (required)</label>' +
                      '<textarea id="pr-r" rows="3" style="width:100%;"></textarea>' +
                      '<div class="action-panel-buttons" style="margin-top: 0.75rem;">' +
                      '  <button class="btn-secondary" id="pr-cancel">Cancel</button>' +
                      '  <button class="btn-primary" id="pr-confirm">Confirm</button>' +
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

  function promptRationaleAndConfirmation(title, warning, ackLabel) {
    return new Promise((resolve) => {
      const div = document.createElement('div');
      div.innerHTML = '<div class="action-panel-high-impact">' + esc(warning) + '</div>' +
                      '<label style="margin-top: 0.75rem;">Rationale (required)</label>' +
                      '<textarea id="prc-r" rows="3" style="width:100%;"></textarea>' +
                      '<label style="display:block; margin-top: 0.5rem;">' +
                      '  <input type="checkbox" id="prc-ack"> ' + esc(ackLabel) +
                      '</label>' +
                      '<div class="action-panel-buttons" style="margin-top: 0.75rem;">' +
                      '  <button class="btn-secondary" id="prc-cancel">Cancel</button>' +
                      '  <button class="btn-danger" id="prc-confirm">Confirm</button>' +
                      '</div>';
      const handle = global.AuroraModal.open({ title: title, body: div, onClose: () => resolve(null) });
      div.querySelector('#prc-cancel').addEventListener('click', () => { handle.close(); });
      div.querySelector('#prc-confirm').addEventListener('click', () => {
        const v = div.querySelector('#prc-r').value.trim();
        const ack = div.querySelector('#prc-ack').checked;
        if (!v) { global.AuroraToast.warning('Rationale is required.'); return; }
        if (!ack) { global.AuroraToast.warning('You must acknowledge the warning.'); return; }
        handle.close();
        resolve(v);
      });
    });
  }

  // ---------- Rail panels ----------

  async function loadSubjectContext() {
    const rail = document.getElementById('ad-rail');
    if (!rail) return;
    rail.insertAdjacentHTML('beforeend', railCard('subject-context', 'Subject context',
      '<p class="empty-state">Loading…</p>'));
    rail.insertAdjacentHTML('beforeend', railCard('records-authored', 'Records authored',
      '<p class="empty-state">Loading…</p>'));
    rail.insertAdjacentHTML('beforeend', railCard('blob-inventory', 'Blob inventory',
      '<p class="empty-state">Loading…</p>'));
    rail.insertAdjacentHTML('beforeend', railCard('invite-lineage', 'Invite lineage',
      '<p class="empty-state">Loading…</p>'));

    try {
      const ctx = await global.AuroraEndpoints.moderator.getSubjectContext({ subjectDid: currentDid });
      renderSubjectContext(ctx);
    } catch (e) {
      setRailBody('subject-context', '<p class="empty-state">Could not load context.</p>');
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
      const data = await global.AuroraEndpoints.moderator.getSubjectHistory({ subjectDid: currentDid, limit: 25 });
      const items = (data && data.items) || [];
      if (items.length === 0) {
        host.innerHTML = '<p class="empty-state">No prior actions on this account.</p>';
        return;
      }
      const fmt = global.AuroraFormat;
      host.innerHTML = '<ul style="list-style:none; padding:0;">' + items.map((it) =>
        '<li style="padding: 0.5rem 0; border-bottom: 1px solid var(--border-color);">' +
        '<strong>' + esc(it.eventType || it.action || '') + '</strong>' +
        ' by ' + (global.AuroraEntityRef ? global.AuroraEntityRef.account(it.actorDid) : esc(it.actorDid)) +
        ' — ' + (fmt ? esc(fmt.relativeTime(it.createdAt || it.timestamp)) : esc(it.createdAt || '')) +
        (it.id != null ? ' · ' + (global.AuroraEntityRef ? global.AuroraEntityRef.event(it.id) : '#' + esc(it.id)) : '') +
        '</li>').join('') + '</ul>';
    } catch (e) {
      host.innerHTML = '<p class="empty-state">Could not load history.</p>';
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
      setRailBody('records-authored', '<p class="empty-state">Could not load records.</p>');
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
          (b.size ? ' <span style="color: var(--text-tertiary);">' +
                    (global.AuroraFormat ? global.AuroraFormat.bytes(b.size) : '') + '</span>' : '') +
          '</li>').join('') + '</ul>');
    } catch (e) {
      setRailBody('blob-inventory', '<p class="empty-state">Could not load blobs.</p>');
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
          ' <span style="color: var(--text-tertiary);">(' + (c.uses || 0) + '/' + (c.available || 1) + ')</span>' +
          '</li>').join('') + '</ul>');
    } catch (e) {
      setRailBody('invite-lineage', '<p class="empty-state">Could not load invites.</p>');
    }
  }

  // ---------- Rail helpers ----------

  function railCard(id, title, body) {
    return '<div class="rail-card" data-rail-id="' + id + '">' +
           '  <h4>' + esc(title) + '</h4>' +
           '  <div data-rail-body>' + body + '</div>' +
           '</div>';
  }

  function setRailBody(id, html) {
    const card = document.querySelector('[data-rail-id="' + id + '"]');
    if (!card) return;
    const body = card.querySelector('[data-rail-body]');
    if (body) body.innerHTML = html;
  }

  function sectionTitle(t) {
    return '<h5 style="margin: 0.75rem 0 0.25rem 0; font-size: 0.75rem; ' +
           'text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-tertiary);">' +
           esc(t) + '</h5>';
  }

  function listOrEmpty(items) {
    if (!items || items.length === 0) return '<p style="color: var(--text-tertiary); font-size: 0.8125rem;">None</p>';
    return '<ul style="list-style:none; padding:0; font-size: 0.875rem;">' + items.join('') + '</ul>';
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }

  if (global.AuroraRouter) global.AuroraRouter.register('opsAccountDetail', { mount: mount });
})(window);
