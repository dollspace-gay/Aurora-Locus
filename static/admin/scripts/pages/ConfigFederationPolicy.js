// Configuration → Federation policy page (route: #configuration/federation-policy).
//
// A read-only status/overview surface (#344, Arc G stub activation; recon
// docs/internal/v09/federation_policy_recon_findings.md). Recovery-Mode (#276)
// / #342 / #343 pattern: all federation config is static env/startup config
// (PDS_FEDERATION_*), so the page surfaces what's true + documents the
// restart-time change procedure + links to Operations → Federation for live
// status. No fake controls; runtime-mutable federation policy is future-cycle.
//
// Data sources:
//   - Sections 1-8 (full deployment config, incl. the peer allowlist +
//     auto-stream toggle): tools.aurora.ops.getFederationPolicy (SuperAdmin).
//   - Section 9 (peer-visible posture): the two PUBLIC describe endpoints
//     themselves — com.atproto.server.describeServer (minimal) and
//     com.aurora.federation.describePosture (richer) — so the SuperAdmin sees
//     exactly what peers actually receive, not a client-side reconstruction.
// v0.9 Federation Pattern-1 Phase B (#352): the trusted peer allowlist is now
// runtime-mutable. SuperAdmins get add / edit / remove affordances on the peer
// section (tools.aurora.ops.{add,remove,modify}FederationPeer); all other
// sections remain read-only env/restart config. Recovery mode greys the
// mutation affordances (substrate refuses with 503 RecoveryModeActive).
// 4xx validation errors surface inline near the form; 5xx (CAS-exhausted,
// recovery) surface as a toast with retry guidance.
// Manual Refresh (no auto-poll). Per-section reads isolated.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  function onoff(b) { return b ? 'Enabled' : 'Disabled'; }

  // Page-scoped state resolved at mount/load: SuperAdmin can mutate; recovery
  // mode locks mutations out.
  let isSuper = false;
  let recoveryActive = false;

  // A read-only status card: title, a value slot (filled by load), and a
  // how-to-change note.
  function card(title, valueId, note) {
    return '<div class="settings-card">' +
      '  <h3>' + esc(title) + '</h3>' +
      '  <div id="' + valueId + '">Loading…</div>' +
      (note ? '  <p class="settings-help">' + note + '</p>' : '') +
      '</div>';
  }

  async function mount({ container }) {
    const session = global.AuroraSession;
    isSuper = !!(session && session.hasRole('superadmin'));
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Federation policy</nav>' +
      '<header class="page-header"><div><h2>Federation policy</h2>' +
      '<p class="page-subtitle">The deployment\'s federation posture (read-only)</p></div>' +
      '<button type="button" class="btn-secondary" id="fed-refresh">Refresh</button></header>' +
      '<p class="settings-help">For live federation status (peer count, recent events, last activity), see <a href="#ops/federation">Operations → Federation</a>.</p>' +
      '<div class="settings-grid">' +
      card('Federation', 'fed-enabled', 'Set via <code>PDS_FEDERATION_ENABLED</code> at startup; restart required to change.') +
      card('Relay binding', 'fed-relays', 'Set via <code>PDS_FEDERATION_RELAY_URLS</code> (comma-separated) at startup; restart required. Live status: Operations → Federation.') +
      card('AppView URL', 'fed-appview', 'Set via <code>PDS_APPVIEW_URL</code> at startup.') +
      card('Trusted peer allowlist', 'fed-peers', 'Seeded at boot from <code>PDS_FEDERATION_PEER_PDS</code> (<code>did@url,…</code>); now runtime-mutable below (SuperAdmin). Controls the trusted-issuer allowlist and discovery bootstrap.') +
      card('Firehose', 'fed-firehose', 'Set via <code>PDS_FEDERATION_FIREHOSE_ENABLED</code> at startup.') +
      card('Relay crawl', 'fed-crawl', 'Set via <code>PDS_FEDERATION_CRAWL_ENABLED</code> at startup.') +
      card('Auto-stream events', 'fed-autostream', 'Set via <code>PDS_FEDERATION_AUTO_STREAM</code> at startup.') +
      card('Public URL', 'fed-public', 'Set via <code>PDS_PUBLIC_URL</code> at startup; this PDS\'s internet-reachable URL.') +
      '</div>' +
      // v0.9 Phase B (#352) — runtime-mutable trusted-peer management.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Manage trusted peers <span class="role-tag">SuperAdmin only</span></h3>' +
      '  <div id="fed-recovery-banner" style="display:none; padding:0.5rem; border-left:3px solid #b45309; background:#fef3c7; margin-bottom:0.6rem;">Federation policy mutations are disabled during recovery mode.</div>' +
      '  <p class="settings-help">Add, edit, or remove trusted peer PDS entries. Changes take effect immediately (per-call freshness) and are written to the audit chain.</p>' +
      '  <div id="fed-peers-manage">' + (isSuper ? 'Loading…' : '<p class="settings-help">SuperAdmin role required to manage trusted peers.</p>') + '</div>' +
      (isSuper ?
        '  <div id="fed-peer-error" style="margin:0.4rem 0;"></div>' +
        '  <fieldset id="fed-peer-form" style="margin-top:0.5rem;"><legend id="fed-peer-legend">Add peer</legend>' +
        '    <input type="hidden" id="fed-peer-edit-did" value="">' +
        '    <label style="display:block;">DID <input type="text" id="fed-peer-did" placeholder="did:plc:…" style="width:100%;"></label>' +
        '    <label style="display:block;">URL (https only) <input type="text" id="fed-peer-url" placeholder="https://…" style="width:100%;"></label>' +
        '    <button type="button" class="btn-primary" id="fed-peer-save">Save peer</button>' +
        '    <button type="button" id="fed-peer-cancel" style="display:none;">Cancel edit</button>' +
        '  </fieldset>'
        : '') +
      '</section>' +
      // v0.9 Phase C (#353) — discovery mode + pending-discovery surface.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Peer discovery <span class="role-tag">SuperAdmin only</span></h3>' +
      '  <p class="settings-help">How peers discovered via relays are handled. <strong>Allowlist-only</strong>: surface for review below. <strong>Auto-accept</strong>: trust automatically. <strong>Disabled</strong>: skip scheduled scans.</p>' +
      (isSuper ?
        '  <label style="display:block;">Discovery mode ' +
        '    <select id="fed-discovery-mode">' +
        '      <option value="allowlist-only">Allowlist-only (surface for review)</option>' +
        '      <option value="auto-accept">Auto-accept (trust discovered peers)</option>' +
        '      <option value="discovery-disabled">Disabled (no scheduled discovery)</option>' +
        '    </select></label>' +
        '  <div id="fed-discovery-warning" style="display:none; padding:0.5rem; border-left:3px solid #b45309; background:#fef3c7; margin:0.4rem 0;">' +
        '<strong>Auto-accept delegates trust to your relays.</strong> Any peer a relay reports will be trusted for federation without review. Use only with relays you fully trust.</div>'
        : '  <p class="settings-help">SuperAdmin role required to manage discovery.</p>') +
      '  <h4 style="margin-top:0.8rem;">Pending discoveries</h4>' +
      '  <p class="settings-help">Peers seen during scans, awaiting review. Bounded to the 100 most-recently-seen.</p>' +
      '  <div id="fed-pending-list">Loading…</div>' +
      '</section>' +
      // v0.9 Phase D (#354) — runtime-mutable relay set.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Relay servers <span class="role-tag">SuperAdmin only</span></h3>' +
      '  <div id="fed-bootseed-banner" style="display:none; padding:0.5rem; border-left:3px solid #b91c1c; background:#fee2e2; margin-bottom:0.6rem;"></div>' +
      '  <p class="settings-help">The relays this PDS connects to for the firehose. Changes take effect immediately (the firehose respawns against the new set). At least 1, at most 10.</p>' +
      '  <div id="fed-relays-manage">' + (isSuper ? 'Loading…' : '<p class="settings-help">SuperAdmin role required to manage relays.</p>') + '</div>' +
      (isSuper ?
        '  <div id="fed-relay-error" style="margin:0.4rem 0;"></div>' +
        '  <fieldset id="fed-relay-form" style="margin-top:0.5rem;"><legend>Add relay</legend>' +
        '    <label style="display:block;">URL (https only) <input type="text" id="fed-relay-url" placeholder="https://…" style="width:100%;"></label>' +
        '    <button type="button" class="btn-primary" id="fed-relay-add">Add relay</button>' +
        '  </fieldset>' +
        '  <fieldset id="fed-relay-switch-form" style="margin-top:0.5rem;"><legend>Replace entire relay set</legend>' +
        '    <label style="display:block;">Relay URLs (one per line) <textarea id="fed-relay-switch-list" rows="3" style="width:100%;" placeholder="https://relay1\\nhttps://relay2"></textarea></label>' +
        '    <label style="display:block;">Transition mode ' +
        '      <select id="fed-relay-transition"><option value="graceful">graceful</option><option value="abrupt">abrupt</option></select></label>' +
        '    <p class="settings-help" title="In v0.9 both modes perform the same firehose-respawn switch; your selection is recorded in the audit log. Reserved for future connection-draining work.">Transition mode is recorded in the audit log; both modes behave identically in v0.9.</p>' +
        '    <button type="button" class="btn-primary" id="fed-relay-switch">Replace relay set</button>' +
        '  </fieldset>'
        : '') +
      '</section>' +
      // Section 9 — what peers actually see (read from the public endpoints).
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Peer-visible posture</h3>' +
      '  <p class="settings-help">What this deployment advertises to federated peers. Read live from the public describe endpoints so it reflects exactly what peers receive.</p>' +
      '  <div class="settings-grid">' +
      '    <div class="settings-card"><h3>Standard discovery</h3>' +
      '      <p class="settings-help"><code>com.atproto.server.describeServer</code> — any ATProto peer.</p>' +
      '      <pre id="fed-describe-server" class="chain-indicator-cmd">Loading…</pre></div>' +
      '    <div class="settings-card"><h3>Aurora-aware discovery</h3>' +
      '      <p class="settings-help"><code>com.aurora.federation.describePosture</code> — Aurora-aware tooling; richer federation detail.</p>' +
      '      <pre id="fed-describe-posture" class="chain-indicator-cmd">Loading…</pre></div>' +
      '  </div>' +
      '</section>' +
      // Future-cycle pointer — honest deferral framing (#342/#343 shape).
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Coming in a future cycle</h3>' +
      '  <p class="settings-help">A runtime-mutable federation <code>enabled</code> toggle, relay health monitoring, and peer reputation are reserved for a later release. The trusted-peer allowlist, discovery mode, and relay set above are all now runtime-mutable; the remaining federation flags still require editing environment variables and restarting the substrate.</p>' +
      '</section>';

    const btn = document.getElementById('fed-refresh');
    if (btn) btn.addEventListener('click', loadAll);
    if (isSuper) {
      const save = document.getElementById('fed-peer-save');
      if (save) save.addEventListener('click', savePeer);
      const cancel = document.getElementById('fed-peer-cancel');
      if (cancel) cancel.addEventListener('click', resetPeerForm);
      const modeSel = document.getElementById('fed-discovery-mode');
      if (modeSel) modeSel.addEventListener('change', onModeChange);
      const relayAdd = document.getElementById('fed-relay-add');
      if (relayAdd) relayAdd.addEventListener('click', addRelay);
      const relaySwitch = document.getElementById('fed-relay-switch');
      if (relaySwitch) relaySwitch.addEventListener('click', switchRelays);
    }
    await loadAll();
    return {};
  }

  async function loadAll() {
    await detectRecovery();
    await Promise.all([loadPolicy(), loadDiscovery(), loadPeerVisible()]);
  }

  // Recovery mode is detected via the substrate signal the moderation-mode
  // setting already exposes (source === 'RecoveryMode'), mirroring the
  // Recovery-mode status page. When active, mutation affordances are greyed.
  async function detectRecovery() {
    recoveryActive = false;
    if (!isSuper) return;
    try {
      const d = await global.AuroraEndpoints.admin.getRuntimeSetting('moderation-mode');
      recoveryActive = !!(d && d.source === 'RecoveryMode');
    } catch (e) { /* leave false; the substrate still enforces server-side */ }
    const banner = document.getElementById('fed-recovery-banner');
    if (banner) banner.style.display = recoveryActive ? '' : 'none';
    const form = document.getElementById('fed-peer-form');
    if (form) {
      form.querySelectorAll('input,button').forEach(function (el) { el.disabled = recoveryActive; });
    }
    const modeSel = document.getElementById('fed-discovery-mode');
    if (modeSel) modeSel.disabled = recoveryActive;
    ['fed-relay-form', 'fed-relay-switch-form'].forEach(function (id) {
      const f = document.getElementById(id);
      if (f) f.querySelectorAll('input,button,textarea,select').forEach(function (el) { el.disabled = recoveryActive; });
    });
  }

  // Sections 1-8 — the SuperAdmin full env view.
  async function loadPolicy() {
    const set = (id, html) => { const el = document.getElementById(id); if (el) el.innerHTML = html; };
    let p;
    try {
      p = await global.AuroraEndpoints.ops.getFederationPolicy();
    } catch (e) {
      for (const id of ['fed-enabled', 'fed-relays', 'fed-appview', 'fed-peers', 'fed-firehose', 'fed-crawl', 'fed-autostream', 'fed-public']) {
        set(id, '<strong>Unavailable</strong>');
      }
      return;
    }
    set('fed-enabled', '<strong>' + esc(onoff(p.enabled)) + '</strong>');
    const relays = Array.isArray(p.relayUrls) ? p.relayUrls : [];
    set('fed-relays', relays.length
      ? '<ul>' + relays.map((u) => '<li><code>' + esc(u) + '</code></li>').join('') + '</ul>'
      : '<em>No relays bound.</em>');
    set('fed-appview', p.appviewUrl ? '<code>' + esc(p.appviewUrl) + '</code>' : '<em>Not configured.</em>');
    const peers = Array.isArray(p.peerPds) ? p.peerPds : [];
    set('fed-peers', peers.length
      ? '<ul>' + peers.map((x) => '<li><code>' + esc(x.did) + '</code> @ <code>' + esc(x.url) + '</code></li>').join('') + '</ul>'
      : '<em>No trusted peers configured.</em>');
    if (isSuper) renderPeerManagement(peers);
    set('fed-firehose', '<strong>' + esc(onoff(p.firehoseEnabled)) + '</strong>');
    set('fed-crawl', '<strong>' + esc(onoff(p.crawlEnabled)) + '</strong>');
    set('fed-autostream', '<strong>' + esc(onoff(p.autoStreamEvents)) + '</strong>');
    set('fed-public', p.publicUrl ? '<code>' + esc(p.publicUrl) + '</code>' : '<em>Not configured.</em>');
    if (isSuper) {
      renderRelayManagement(relays);
      renderBootSeedBanner(p.bootSeedStatus);
    }
  }

  // Phase D — the SuperAdmin editable relay list + boot-seed-failure banner.
  function renderRelayManagement(relays) {
    const host = document.getElementById('fed-relays-manage');
    if (!host) return;
    if (!relays.length) { host.innerHTML = '<p class="settings-help">No relays bound.</p>'; return; }
    host.innerHTML = relays.map(function (u) {
      return '<div class="hook-row" style="border-bottom:1px solid #ddd; padding:0.3rem 0;">' +
        '<code>' + esc(u) + '</code>' +
        ' <button type="button" class="fed-relay-remove" data-url="' + esc(u) + '"' + (recoveryActive ? ' disabled' : '') + '>Remove</button>' +
        '</div>';
    }).join('');
    host.querySelectorAll('.fed-relay-remove').forEach(function (b) {
      b.addEventListener('click', function () { removeRelay(b.getAttribute('data-url')); });
    });
    // Pre-fill the switch textarea with the current set for convenience.
    const ta = document.getElementById('fed-relay-switch-list');
    if (ta && !ta.value) ta.value = relays.join('\n');
  }

  function renderBootSeedBanner(status) {
    const banner = document.getElementById('fed-bootseed-banner');
    if (!banner) return;
    if (status && status.bootSeedFailed) {
      const keys = (status.failedKeys || []).map(esc).join(', ');
      banner.innerHTML = '<strong>Boot-seed failure — federation policy mutations are disabled.</strong> ' +
        'Failed keys: <code>' + keys + '</code>. Inspect the audit log, correct configuration, and restart the substrate.';
      banner.style.display = '';
    } else {
      banner.style.display = 'none';
    }
  }

  function clearRelayError() {
    const el = document.getElementById('fed-relay-error');
    if (el) el.innerHTML = '';
  }

  function handleRelayError(e, fallback) {
    const status = e && e.status;
    const msg = (e && e.message) ? e.message : fallback;
    if (status && status >= 400 && status < 500) {
      const el = document.getElementById('fed-relay-error');
      if (el && global.AuroraInlineError) { el.innerHTML = global.AuroraInlineError.render({ message: msg }); }
      else if (el) { el.innerHTML = '<p class="settings-help" style="color:#b91c1c;">' + esc(msg) + '</p>'; }
      else { global.AuroraToast.danger(msg); }
    } else {
      global.AuroraToast.danger(msg + ' — retry shortly.');
    }
  }

  async function addRelay() {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const url = (document.getElementById('fed-relay-url').value || '').trim();
    clearRelayError();
    if (!url) { global.AuroraToast.warning('URL is required.'); return; }
    try {
      await global.AuroraEndpoints.ops.addRelayUrl({ url: url });
      global.AuroraToast.success('Relay added; firehose respawning.');
      document.getElementById('fed-relay-url').value = '';
      await loadPolicy();
    } catch (e) { handleRelayError(e, 'Add failed.'); }
  }

  async function removeRelay(url) {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const r = await global.AuroraModal.destructiveConfirm({
      heading: 'Remove relay',
      body: 'Remove ' + url + ' from the relay set? The firehose will respawn against the remaining relays.',
      confirmLabel: 'Remove relay',
    });
    if (!r.confirmed) return;
    try {
      await global.AuroraEndpoints.ops.removeRelayUrl({ url: url });
      global.AuroraToast.success('Relay removed.');
      await loadPolicy();
    } catch (e) { handleRelayError(e, 'Remove failed.'); }
  }

  async function switchRelays() {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const raw = (document.getElementById('fed-relay-switch-list').value || '').trim();
    const mode = (document.getElementById('fed-relay-transition') || {}).value || 'graceful';
    clearRelayError();
    const urls = raw.split('\n').map(function (s) { return s.trim(); }).filter(Boolean);
    if (!urls.length) { global.AuroraToast.warning('At least 1 relay URL is required.'); return; }
    try {
      await global.AuroraEndpoints.ops.setFederationRelays({ relayUrls: urls, transitionMode: mode });
      global.AuroraToast.success('Relay set replaced; firehose respawning.');
      await loadPolicy();
    } catch (e) { handleRelayError(e, 'Replace failed.'); }
  }

  // Phase B — the SuperAdmin editable peer list with per-row edit/remove.
  function renderPeerManagement(peers) {
    const host = document.getElementById('fed-peers-manage');
    if (!host) return;
    if (!peers.length) { host.innerHTML = '<p class="settings-help">No trusted peers. Add one below.</p>'; return; }
    host.innerHTML = peers.map(function (x) {
      return '<div class="hook-row" style="border-bottom:1px solid #ddd; padding:0.3rem 0;">' +
        '<code>' + esc(x.did) + '</code> @ <code>' + esc(x.url) + '</code>' +
        ' <button type="button" class="fed-peer-edit" data-did="' + esc(x.did) + '" data-url="' + esc(x.url) + '"' + (recoveryActive ? ' disabled' : '') + '>Edit</button>' +
        ' <button type="button" class="fed-peer-remove" data-did="' + esc(x.did) + '"' + (recoveryActive ? ' disabled' : '') + '>Remove</button>' +
        '</div>';
    }).join('');
    host.querySelectorAll('.fed-peer-edit').forEach(function (b) {
      b.addEventListener('click', function () { editPeer(b.getAttribute('data-did'), b.getAttribute('data-url')); });
    });
    host.querySelectorAll('.fed-peer-remove').forEach(function (b) {
      b.addEventListener('click', function () { removePeer(b.getAttribute('data-did')); });
    });
  }

  function clearPeerError() {
    const el = document.getElementById('fed-peer-error');
    if (el) el.innerHTML = '';
  }

  // 4xx validation errors render inline near the form (memory: error→inline);
  // 5xx (CAS-exhausted, recovery 503) surface as a toast with retry guidance.
  function handlePeerError(e, fallback) {
    const status = e && e.status;
    const msg = (e && e.message) ? e.message : fallback;
    if (status && status >= 400 && status < 500) {
      const el = document.getElementById('fed-peer-error');
      if (el && global.AuroraInlineError) {
        el.innerHTML = global.AuroraInlineError.render({ message: msg });
      } else if (el) {
        el.innerHTML = '<p class="settings-help" style="color:#b91c1c;">' + esc(msg) + '</p>';
      } else {
        global.AuroraToast.danger(msg);
      }
    } else {
      global.AuroraToast.danger(msg + ' — retry shortly.');
    }
  }

  function resetPeerForm() {
    const did = document.getElementById('fed-peer-did');
    const url = document.getElementById('fed-peer-url');
    const editDid = document.getElementById('fed-peer-edit-did');
    const legend = document.getElementById('fed-peer-legend');
    const cancel = document.getElementById('fed-peer-cancel');
    if (editDid) editDid.value = '';
    if (legend) legend.textContent = 'Add peer';
    if (cancel) cancel.style.display = 'none';
    if (did) { did.value = ''; did.disabled = recoveryActive; }
    if (url) url.value = '';
    clearPeerError();
  }

  function editPeer(did, url) {
    document.getElementById('fed-peer-edit-did').value = did;
    document.getElementById('fed-peer-legend').textContent = 'Edit peer URL';
    document.getElementById('fed-peer-cancel').style.display = '';
    const didEl = document.getElementById('fed-peer-did');
    didEl.value = did;
    didEl.disabled = true; // DID is the key; only the URL is editable on modify.
    document.getElementById('fed-peer-url').value = url;
    clearPeerError();
  }

  async function savePeer() {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const ep = global.AuroraEndpoints;
    const editDid = document.getElementById('fed-peer-edit-did').value;
    const did = document.getElementById('fed-peer-did').value.trim();
    const url = document.getElementById('fed-peer-url').value.trim();
    clearPeerError();
    if (!url) { global.AuroraToast.warning('URL is required.'); return; }
    try {
      if (editDid) {
        await ep.ops.modifyFederationPeer({ did: editDid, newUrl: url });
        global.AuroraToast.success('Peer URL updated.');
      } else {
        if (!did) { global.AuroraToast.warning('DID is required.'); return; }
        await ep.ops.addFederationPeer({ did: did, url: url });
        global.AuroraToast.success('Peer added.');
      }
      resetPeerForm();
      await loadPolicy();
    } catch (e) {
      handlePeerError(e, 'Save failed.');
    }
  }

  async function removePeer(did) {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const confirmResult = await global.AuroraModal.destructiveConfirm({
      heading: 'Remove trusted peer',
      body: 'Remove ' + did + ' from the trusted-peer allowlist? Federation trust stops immediately.',
      confirmLabel: 'Remove peer',
    });
    if (!confirmResult.confirmed) return;
    try {
      await global.AuroraEndpoints.ops.removeFederationPeer({ did: did });
      global.AuroraToast.success('Peer removed.');
      await loadPolicy();
    } catch (e) {
      handlePeerError(e, 'Remove failed.');
    }
  }

  // Phase C — discovery mode + pending-discovery surface, read via the generic
  // getRuntimeSetting (the describe composite-load is Phase E's job).
  async function loadDiscovery() {
    const ep = global.AuroraEndpoints;
    if (!ep || !isSuper) return;
    let mode = 'allowlist-only';
    try {
      const d = await ep.admin.getRuntimeSetting('federation.policy.discovery-mode');
      if (d && typeof d.value === 'string') mode = d.value;
    } catch (e) { /* default */ }
    const sel = document.getElementById('fed-discovery-mode');
    if (sel) sel.value = mode;
    toggleAutoAcceptWarning(mode);

    let pending = [];
    try {
      const d = await ep.admin.getRuntimeSetting('federation.policy.pending-discoveries');
      if (d && Array.isArray(d.value)) pending = d.value;
    } catch (e) { /* empty */ }
    renderPending(pending);
  }

  function toggleAutoAcceptWarning(mode) {
    const warn = document.getElementById('fed-discovery-warning');
    if (warn) warn.style.display = mode === 'auto-accept' ? '' : 'none';
  }

  function renderPending(pending) {
    const host = document.getElementById('fed-pending-list');
    if (!host) return;
    if (!pending.length) { host.innerHTML = '<p class="settings-help">No pending peer discoveries.</p>'; return; }
    if (!isSuper) {
      host.innerHTML = '<ul>' + pending.map((p) => '<li><code>' + esc(p.did) + '</code> @ <code>' + esc(p.url) + '</code></li>').join('') + '</ul>';
      return;
    }
    host.innerHTML = pending.map(function (p) {
      return '<div class="hook-row" style="border-bottom:1px solid #ddd; padding:0.3rem 0;">' +
        '<code>' + esc(p.did) + '</code> @ <code>' + esc(p.url) + '</code>' +
        ' <span class="settings-help">first seen ' + esc(p.first_seen_at || '') + ', last seen ' + esc(p.last_seen_at || '') + '</span>' +
        ' <button type="button" class="fed-pending-accept" data-did="' + esc(p.did) + '" data-url="' + esc(p.url) + '"' + (recoveryActive ? ' disabled' : '') + '>Accept</button>' +
        ' <button type="button" class="fed-pending-dismiss" data-did="' + esc(p.did) + '"' + (recoveryActive ? ' disabled' : '') + '>Dismiss</button>' +
        '</div>';
    }).join('');
    host.querySelectorAll('.fed-pending-accept').forEach(function (b) {
      b.addEventListener('click', function () { acceptPending(b.getAttribute('data-did'), b.getAttribute('data-url')); });
    });
    host.querySelectorAll('.fed-pending-dismiss').forEach(function (b) {
      b.addEventListener('click', function () { dismissPending(b.getAttribute('data-did')); });
    });
  }

  async function onModeChange() {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const sel = document.getElementById('fed-discovery-mode');
    const mode = sel ? sel.value : 'allowlist-only';
    // Confirm the trust-delegation risk when switching TO auto-accept.
    if (mode === 'auto-accept') {
      const r = await global.AuroraModal.destructiveConfirm({
        heading: 'Switch to auto-accept?',
        body: 'Auto-accept trusts any peer your relays report, without review. Use only with fully-trusted relays.',
        confirmLabel: 'Enable auto-accept',
      });
      if (!r.confirmed) { await loadDiscovery(); return; }
    }
    try {
      await global.AuroraEndpoints.ops.setDiscoveryMode({ mode: mode });
      global.AuroraToast.success('Discovery mode updated.');
      toggleAutoAcceptWarning(mode);
    } catch (e) {
      global.AuroraToast.danger('Mode change failed: ' + (e && e.message ? e.message : '') + ' — retry shortly.');
      await loadDiscovery();
    }
  }

  async function acceptPending(did, url) {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    try {
      await global.AuroraEndpoints.ops.addFederationPeer({ did: did, url: url });
      global.AuroraToast.success('Peer accepted into the allowlist.');
      await Promise.all([loadPolicy(), loadDiscovery()]);
    } catch (e) {
      global.AuroraToast.danger('Accept failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function dismissPending(did) {
    if (recoveryActive) { global.AuroraToast.danger('Disabled during recovery mode.'); return; }
    const r = await global.AuroraModal.destructiveConfirm({
      heading: 'Dismiss pending discovery',
      body: 'Remove ' + did + ' from the pending list? It may re-surface on a future scan.',
      confirmLabel: 'Dismiss',
    });
    if (!r.confirmed) return;
    try {
      await global.AuroraEndpoints.ops.dismissPendingDiscovery({ did: did });
      global.AuroraToast.success('Pending discovery dismissed.');
      await loadDiscovery();
    } catch (e) {
      global.AuroraToast.danger('Dismiss failed: ' + (e && e.message ? e.message : ''));
    }
  }

  // Section 9 — render the two public endpoints' actual responses (the
  // federation block of describeServer; the full describePosture).
  async function loadPeerVisible() {
    const ds = document.getElementById('fed-describe-server');
    try {
      const d = await global.AuroraEndpoints.atproto.describeServer();
      const fed = (d && d.federation) || {};
      if (ds) ds.textContent = JSON.stringify(fed, null, 2);
    } catch (e) {
      if (ds) ds.textContent = 'Unavailable';
    }
    const dp = document.getElementById('fed-describe-posture');
    try {
      const p = await global.AuroraEndpoints.atproto.describeFederationPosture();
      if (dp) dp.textContent = JSON.stringify(p || {}, null, 2);
    } catch (e) {
      if (dp) dp.textContent = 'Unavailable';
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configFederationPolicy', { mount: mount });
})(window);
