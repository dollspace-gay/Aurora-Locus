// System health page (route: #ops/system-health).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.6.5.

(function (global) {
  'use strict';

  let pollHandle = null;

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#dashboard">Operations</a> <span class="breadcrumb-sep">›</span> System health</nav>' +
      '<header class="page-header"><div><h2>System health</h2><p class="page-subtitle">Subsystem status, jobs, validation</p></div></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card" id="sh-overall"><h3>Overall</h3><p class="empty-state">Loading…</p></div>' +
      '  <div class="settings-card" id="sh-resources"><h3>Resource usage</h3><p class="empty-state">Loading…</p></div>' +
      '  <div class="settings-card" id="sh-database"><h3>Database</h3><p class="empty-state">Loading…</p></div>' +
      '  <div class="settings-card" id="sh-jobs"><h3>Background jobs</h3><p class="empty-state">Loading…</p></div>' +
      '  <div class="settings-card" id="sh-nonce"><h3>Nonce store</h3><p class="empty-state">Loading…</p></div>' +
      '  <div class="settings-card" id="sh-validation"><h3>Validation failures</h3><p class="empty-state">Loading…</p></div>' +
      '</div>' +
      '<div class="ops-section">' +
      '  <h3>Controls</h3>' +
      '  <div class="action-panel-buttons" style="justify-content:flex-start; gap: 0.5rem; flex-wrap: wrap;">' +
      '    <button class="btn-secondary" id="sh-checks">Run health checks</button>' +
      '    <button class="btn-secondary" id="sh-cleanup-nonce">Cleanup nonce stores</button>' +
      '  </div>' +
      '</div>';
    document.getElementById('sh-checks').addEventListener('click', async () => {
      try { await global.AuroraEndpoints.ops.runHealthChecks();
            global.AuroraToast.success('Health checks scheduled.'); await refresh(); }
      catch (e) { global.AuroraToast.danger('Run failed: ' + (e && e.message)); }
    });
    document.getElementById('sh-cleanup-nonce').addEventListener('click', async () => {
      const result = await global.AuroraModal.form({
        heading: 'Clean up expired nonce stores?',
        body: 'Removes expired nonce records. Active nonces are unaffected.',
        fields: [],
        submitLabel: 'Clean up',
      });
      if (!result.submitted) return;
      try { await global.AuroraEndpoints.ops.cleanupNonceStores();
            global.AuroraToast.success('Cleanup complete.'); await refresh(); }
      catch (e) { global.AuroraToast.danger('Cleanup failed: ' + (e && e.message)); }
    });
    await refresh();
    pollHandle = setInterval(refresh, 30_000);
    return { unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    safeRender('sh-overall', '<h3>Overall</h3>', async () => {
      const h = await ep.ops.getSystemHealth();
      const subs = h.subsystems || {};
      // §9.6: System Health is the rollup, the Sequencer page is the
      // detail. The sequencer indicator here shows state + tail-lag + a
      // click-through to #ops/sequencer (cursor/controls/raw metrics live
      // there). Pull the lag from getSequencerStatus.
      let seq = null;
      try { seq = await ep.ops.getSequencerStatus(); } catch (e) { /* lag optional */ }
      function seqRollup(statusFallback) {
        const state = (seq && seq.state) || statusFallback || 'unknown';
        const lag = (seq && seq.lagSeconds != null && global.AuroraFormat)
          ? ' · lag ' + esc(global.AuroraFormat.durationCompact(seq.lagSeconds)) : '';
        const badge = global.AuroraStatusBadge ? global.AuroraStatusBadge.render(state, state) : esc(state);
        return '<p><strong>sequencer:</strong> ' + badge + lag +
               ' <a href="#ops/sequencer">Open Sequencer →</a></p>';
      }
      const rows = [];
      let sawSeq = false;
      for (const [k, v] of Object.entries(subs)) {
        if (k === 'sequencer') { sawSeq = true; rows.push(seqRollup(v && v.status)); continue; }
        rows.push('<p><strong>' + esc(k) + ':</strong> ' +
          (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(v.status, v.status) : esc(v.status)) +
          ' ' + esc(v.message || '') + '</p>');
      }
      if (!sawSeq && seq) rows.push(seqRollup());
      return rows.join('') || 'No subsystem data.';
    });
    safeRender('sh-resources', '<h3>Resource usage</h3>', async () => {
      const r = await ep.ops.getResourceUsage();
      return '<p><strong>CPU:</strong> ' + esc(r.cpuPercent || 0) + '%</p>' +
             '<p><strong>Memory:</strong> ' + esc(global.AuroraFormat ? global.AuroraFormat.bytes(r.memoryBytes) : '—') + '</p>' +
             '<p><strong>Disk:</strong> ' + esc(global.AuroraFormat ? global.AuroraFormat.bytes(r.diskBytes) : '—') + '</p>';
    });
    safeRender('sh-database', '<h3>Database</h3>', async () => {
      const d = await ep.ops.getDatabaseStatus();
      return '<p><strong>Backend:</strong> ' + esc(d.backend || '—') + '</p>' +
             '<p><strong>Pool:</strong> ' + esc(d.poolUsed || 0) + ' / ' + esc(d.poolMax || 0) + '</p>' +
             '<p><strong>Latency p99:</strong> ' + esc(d.latencyP99Ms || '—') + 'ms</p>';
    });
    safeRender('sh-jobs', '<h3>Background jobs</h3>', async () => {
      const j = await ep.ops.listBackgroundJobs();
      const jobs = (j && j.jobs) || [];
      if (jobs.length === 0) return '<p class="empty-state">No active jobs.</p>';
      return '<ul style="list-style:none; padding:0;">' + jobs.map((x) =>
        '<li>' + esc(x.name) + ': ' + (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(x.status, x.status) : esc(x.status)) + '</li>').join('') + '</ul>';
    });
    safeRender('sh-nonce', '<h3>Nonce store</h3>', async () => {
      const n = await ep.ops.getNonceStoreStatus();
      return '<p><strong>Active nonces:</strong> ' + esc(n.activeCount || 0) + '</p>' +
             '<p><strong>Expired (cleanable):</strong> ' + esc(n.expiredCount || 0) + '</p>';
    });
    safeRender('sh-validation', '<h3>Validation failures</h3>', async () => {
      const v = await ep.ops.getValidationFailures({ limit: 10 });
      const items = (v && v.failures) || [];
      if (items.length === 0) return '<p class="empty-state">No recent failures.</p>';
      return '<ul style="list-style:none; padding:0;">' + items.map((f) =>
        '<li>' + esc(f.kind) + ': ' + esc(f.message) + '</li>').join('') + '</ul>';
    });
  }

  async function safeRender(id, header, getter) {
    try {
      const html = await getter();
      const c = document.getElementById(id);
      if (c) c.innerHTML = header + (typeof html === 'string' ? html : '<p class="empty-state">No data.</p>');
    } catch (e) {
      const c = document.getElementById(id);
      if (c) c.innerHTML = header + '<p class="empty-state">Unavailable.</p>';
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsSystemHealth', { mount: mount });
})(window);
