// Blob ops page (route: #ops/blob-ops).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.6.3.

(function (global) {
  'use strict';

  let pollHandle = null;

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#dashboard">Operations</a> <span class="breadcrumb-sep">›</span> Blob ops</nav>' +
      '<header class="page-header"><div><h2>Blob ops</h2><p class="page-subtitle">Storage and blob lifecycle</p></div></header>' +
      '<div class="ops-section" id="bo-stats"><h3>Storage statistics</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section" id="bo-recent"><h3>Recent blobs</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section">' +
      '  <h3>Controls</h3>' +
      '  <button class="btn-secondary" id="bo-gc">Run GC</button>' +
      '</div>';
    document.getElementById('bo-gc').addEventListener('click', runGC);
    await refresh();
    pollHandle = setInterval(refresh, 60_000);
    return { unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const stats = await ep.ops.getBlobStatistics();
      const fmt = global.AuroraFormat;
      const byType = (stats.byMimeType || []).map((m) =>
        '<li>' + esc(m.mimeType) + ': ' + esc(m.count) + ' (' + esc(fmt ? fmt.bytes(m.totalSize) : '') + ')</li>').join('');
      document.getElementById('bo-stats').innerHTML = '<h3>Storage statistics</h3>' +
        '<p><strong>Total blobs:</strong> ' + esc(stats.totalBlobs || 0) + '</p>' +
        '<p><strong>Total size:</strong> ' + esc(fmt ? fmt.bytes(stats.totalSize) : '') + '</p>' +
        (byType ? '<p><strong>By mime type:</strong></p><ul>' + byType + '</ul>' : '');
    } catch (e) {
      document.getElementById('bo-stats').innerHTML = '<h3>Storage statistics</h3><p class="empty-state">Unavailable.</p>';
    }
    try {
      const data = await ep.ops.listBlobs({ limit: 25 });
      const blobs = (data && data.blobs) || [];
      const c = document.getElementById('bo-recent');
      if (blobs.length === 0) {
        c.innerHTML = '<h3>Recent blobs</h3><p class="empty-state">No blobs.</p>';
        return;
      }
      const fmt = global.AuroraFormat;
      c.innerHTML = '<h3>Recent blobs</h3>' +
        '<table class="data-table"><thead><tr><th>CID</th><th>Mime</th><th>Size</th><th>Owner</th></tr></thead><tbody>' +
        blobs.map((b) => '<tr>' +
          '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.blob(b.cid) : '<code>' + esc(b.cid) + '</code>') + '</td>' +
          '<td>' + esc(b.mimeType || '—') + '</td>' +
          '<td>' + esc(fmt ? fmt.bytes(b.size) : '—') + '</td>' +
          '<td>' + (b.did ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(b.did) : '<code>' + esc(b.did) + '</code>') : '—') + '</td>' +
          '</tr>').join('') +
        '</tbody></table>';
    } catch (e) {
      document.getElementById('bo-recent').innerHTML = '<h3>Recent blobs</h3><p class="empty-state">Unavailable.</p>';
    }
  }

  async function runGC() {
    const result = await global.AuroraModal.form({
      heading: 'Run blob garbage collection?',
      body: 'This may take a few minutes. The operation is safe to interrupt.',
      fields: [],
      submitLabel: 'Run GC',
    });
    if (!result.submitted) return;
    try {
      await global.AuroraClient.post('tools.aurora.ops.runBlobGC', {});
      global.AuroraToast.success('Blob GC started.');
      await refresh();
    } catch (e) {
      global.AuroraToast.danger('GC failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsBlobOps', { mount: mount });
})(window);
