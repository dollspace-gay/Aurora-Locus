// Federation ops page (route: #ops/federation).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.6.2.

(function (global) {
  'use strict';

  let pollHandle = null;

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#dashboard">Operations</a> <span class="breadcrumb-sep">›</span> Federation</nav>' +
      '<header class="page-header"><div><h2>Federation</h2><p class="page-subtitle">Relays, peers, federation activity</p></div></header>' +
      '<div class="ops-section" id="fed-relay"><h3>Relay configuration</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section" id="fed-status"><h3>Federation status</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section" id="fed-peers"><h3>Known instances</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section">' +
      '  <h3>Controls</h3>' +
      '  <button class="btn-secondary" id="fed-discover">Trigger discovery</button>' +
      '</div>';
    document.getElementById('fed-discover').addEventListener('click', triggerDiscovery);
    await refresh();
    pollHandle = setInterval(refresh, 60_000);
    return { unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const relay = await ep.ops.getRelayConfig();
      document.getElementById('fed-relay').innerHTML = '<h3>Relay configuration</h3>' +
        '<p><strong>Relay:</strong> ' + esc(relay.relayUrl || relay.url || 'unconfigured') + '</p>' +
        '<p><strong>Mode:</strong> ' + esc(relay.mode || '—') + '</p>';
    } catch (e) { /* ignore */ }
    try {
      const status = await ep.ops.getFederationStatus();
      document.getElementById('fed-status').innerHTML = '<h3>Federation status</h3>' +
        '<p><strong>Peer count:</strong> ' + esc(status.peerCount || 0) + '</p>' +
        '<p><strong>Recent events:</strong> ' + esc(status.recentEventCount || 0) + '</p>' +
        '<p><strong>Last activity:</strong> ' + global.AuroraTimestamp.render({ value: status.lastActivityAt, context: 'activity' }) + '</p>';
    } catch (e) {
      document.getElementById('fed-status').innerHTML = '<h3>Federation status</h3><p class="empty-state">Unavailable.</p>';
    }
    try {
      const peers = await ep.ops.listKnownInstances({ limit: 25 });
      const items = (peers && (peers.instances || peers.items)) || [];
      const c = document.getElementById('fed-peers');
      if (items.length === 0) {
        c.innerHTML = '<h3>Known instances</h3><p class="empty-state">No known peers yet.</p>';
        return;
      }
      const fmt = global.AuroraFormat;
      c.innerHTML = '<h3>Known instances</h3>' +
        '<table class="data-table"><thead><tr><th>Hostname</th><th>Last seen</th><th>Status</th></tr></thead><tbody>' +
        items.map((i) => '<tr>' +
          '<td>' + esc(i.hostname || i.url) + '</td>' +
          '<td>' + global.AuroraTimestamp.render({ value: i.lastSeenAt, context: 'activity' }) + '</td>' +
          '<td>' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(i.status || 'active', i.status || 'active') : '') + '</td>' +
          '</tr>').join('') +
        '</tbody></table>';
    } catch (e) {
      document.getElementById('fed-peers').innerHTML = '<h3>Known instances</h3><p class="empty-state">Unavailable.</p>';
    }
  }

  async function triggerDiscovery() {
    try {
      await global.AuroraEndpoints.ops.triggerPdsDiscovery();
      global.AuroraToast.success('Discovery triggered.');
      await refresh();
    } catch (e) {
      global.AuroraToast.danger('Discovery failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsFederation', { mount: mount });
})(window);
