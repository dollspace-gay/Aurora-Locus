// Rate limits page (route: #ops/rate-limits).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.6.4.

(function (global) {
  'use strict';

  let pollHandle = null;

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#dashboard">Operations</a> <span class="breadcrumb-sep">›</span> Rate limits</nav>' +
      '<header class="page-header"><div><h2>Rate limits</h2><p class="page-subtitle">Per-endpoint configuration and current state</p></div></header>' +
      '<div class="ops-section" id="rl-config"><h3>Configuration</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section" id="rl-status"><h3>Status</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section">' +
      '  <h3>Controls</h3>' +
      '  <button class="btn-secondary" id="rl-cleanup">Cleanup state</button>' +
      '</div>';
    document.getElementById('rl-cleanup').addEventListener('click', async () => {
      const result = await global.AuroraModal.form({
        heading: 'Clean up rate-limit state?',
        body: 'Remove expired rate-limit entries.',
        fields: [],
        submitLabel: 'Clean up',
      });
      if (!result.submitted) return;
      try {
        await global.AuroraEndpoints.ops.cleanupRateLimitState();
        global.AuroraToast.success('Rate-limit state cleaned.');
        await refresh();
      } catch (e) {
        global.AuroraToast.danger('Cleanup failed: ' + (e && e.message ? e.message : ''));
      }
    });
    await refresh();
    pollHandle = setInterval(refresh, 30_000);
    return { unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const cfg = await ep.ops.getRateLimitConfig();
      const limits = (cfg && cfg.limits) || [];
      document.getElementById('rl-config').innerHTML = '<h3>Configuration</h3>' +
        (limits.length === 0 ? '<p class="empty-state">No rate limits configured.</p>' :
         '<table class="data-table"><thead><tr><th>Endpoint</th><th>Limit</th><th>Window</th></tr></thead><tbody>' +
         limits.map((l) => '<tr><td>' + esc(l.endpoint) + '</td><td>' + esc(l.maxRequests) +
           '</td><td>' + esc(l.windowSeconds) + 's</td></tr>').join('') + '</tbody></table>');
    } catch (e) {
      document.getElementById('rl-config').innerHTML = '<h3>Configuration</h3><p class="empty-state">Unavailable.</p>';
    }
    try {
      const status = await ep.ops.getRateLimitStatus();
      const buckets = (status && status.buckets) || [];
      document.getElementById('rl-status').innerHTML = '<h3>Status</h3>' +
        '<p><strong>Tracked identifiers:</strong> ' + esc(status.identifierCount || 0) + '</p>' +
        (buckets.length === 0 ? '<p class="empty-state">No active throttles.</p>' :
         '<table class="data-table"><thead><tr><th>Identifier</th><th>Endpoint</th><th>Used</th></tr></thead><tbody>' +
         buckets.slice(0, 25).map((b) => '<tr><td>' + esc(b.identifier) + '</td><td>' + esc(b.endpoint) +
           '</td><td>' + esc(b.usedRequests) + ' / ' + esc(b.maxRequests) + '</td></tr>').join('') + '</tbody></table>');
    } catch (e) {
      document.getElementById('rl-status').innerHTML = '<h3>Status</h3><p class="empty-state">Unavailable.</p>';
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsRateLimits', { mount: mount });
})(window);
