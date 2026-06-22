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
// Read-only: no setRuntimeSetting, no mutation. Manual Refresh (no auto-poll;
// Operations → Federation owns live polling). Per-section reads isolated.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  function onoff(b) { return b ? 'Enabled' : 'Disabled'; }

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
      card('Trusted peer allowlist', 'fed-peers', 'Set via <code>PDS_FEDERATION_PEER_PDS</code> (<code>did@url,…</code>) at startup; controls the trusted-issuer allowlist and discovery bootstrap. Restart required.') +
      card('Firehose', 'fed-firehose', 'Set via <code>PDS_FEDERATION_FIREHOSE_ENABLED</code> at startup.') +
      card('Relay crawl', 'fed-crawl', 'Set via <code>PDS_FEDERATION_CRAWL_ENABLED</code> at startup.') +
      card('Auto-stream events', 'fed-autostream', 'Set via <code>PDS_FEDERATION_AUTO_STREAM</code> at startup.') +
      card('Public URL', 'fed-public', 'Set via <code>PDS_PUBLIC_URL</code> at startup; this PDS\'s internet-reachable URL.') +
      '</div>' +
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
      '  <p class="settings-help">Runtime-mutable federation policy (peer allow/deny lists, discovery mode, relay reconfiguration without restart) is reserved for a later release. This page surfaces the deployment-configured state; mutation requires editing environment variables and restarting the substrate.</p>' +
      '</section>';

    const btn = document.getElementById('fed-refresh');
    if (btn) btn.addEventListener('click', loadAll);
    await loadAll();
    return {};
  }

  async function loadAll() {
    await Promise.all([loadPolicy(), loadPeerVisible()]);
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
    set('fed-firehose', '<strong>' + esc(onoff(p.firehoseEnabled)) + '</strong>');
    set('fed-crawl', '<strong>' + esc(onoff(p.crawlEnabled)) + '</strong>');
    set('fed-autostream', '<strong>' + esc(onoff(p.autoStreamEvents)) + '</strong>');
    set('fed-public', p.publicUrl ? '<code>' + esc(p.publicUrl) + '</code>' : '<em>Not configured.</em>');
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
