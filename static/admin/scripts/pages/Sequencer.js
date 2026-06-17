// Sequencer ops page (route: #ops/sequencer).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.6.1.

(function (global) {
  'use strict';

  let pollHandle = null;

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#dashboard">Operations</a> <span class="breadcrumb-sep">›</span> Sequencer</nav>' +
      '<header class="page-header"><div><h2>Sequencer</h2><p class="page-subtitle">Position, lag, recent events</p></div></header>' +
      '<div class="ops-section" id="seq-status"><h3>Status</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="ops-section" id="seq-controls"><h3>Controls</h3>' +
      '  <div class="action-panel-buttons" style="justify-content:flex-start; gap: 0.5rem; flex-wrap: wrap;">' +
      '    <button class="btn-secondary" id="seq-pause">Pause</button>' +
      '    <button class="btn-secondary" id="seq-resume">Resume</button>' +
      '    <button class="btn-danger" id="seq-reset">Reset cursor</button>' +
      '    <button class="btn-danger" id="seq-rebuild">Rebuild</button>' +
      '  </div>' +
      '</div>' +
      '<div class="ops-section" id="seq-recent"><h3>Recent events</h3><p class="empty-state">Loading…</p></div>';

    document.getElementById('seq-pause').addEventListener('click', () => doAction('pauseSequencer', 'Pause sequencer?'));
    document.getElementById('seq-resume').addEventListener('click', () => doAction('resumeSequencer', 'Resume sequencer?'));
    document.getElementById('seq-reset').addEventListener('click', () => doAction('resetSequencerCursor', 'Reset sequencer cursor? This is high-impact.'));
    document.getElementById('seq-rebuild').addEventListener('click', () => doAction('rebuildSequencer', 'Rebuild the entire sequencer? This is very high-impact.'));

    await refresh();
    pollHandle = setInterval(refresh, 30_000);
    return { unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const status = await ep.ops.getSequencerStatus();
      const c = document.getElementById('seq-status');
      const fmt = global.AuroraFormat;
      c.innerHTML = '<h3>Status</h3>' +
        '<p><strong>State:</strong> ' + esc(status.state || 'unknown') + '</p>' +
        '<p><strong>Position:</strong> ' + esc(status.position) + '</p>' +
        '<p><strong>Lag:</strong> ' + esc(status.lagSeconds != null ? fmt.durationCompact(status.lagSeconds) : '—') + '</p>' +
        '<p><strong>Events/sec:</strong> ' + esc(status.eventsPerSecond || 0) + '</p>';
    } catch (e) {
      document.getElementById('seq-status').innerHTML = '<h3>Status</h3><p class="empty-state">Unavailable.</p>';
    }
    try {
      const recent = await ep.atproto.listRecentEvents({ limit: 20 });
      const items = (recent && (recent.events || recent.items)) || [];
      const c = document.getElementById('seq-recent');
      if (items.length === 0) {
        c.innerHTML = '<h3>Recent events</h3><p class="empty-state">No recent events.</p>';
        return;
      }
      const fmt = global.AuroraFormat;
      c.innerHTML = '<h3>Recent events</h3>' +
        '<table class="data-table"><thead><tr><th>Seq</th><th>Type</th><th>When</th><th>Repo</th></tr></thead>' +
        '<tbody>' + items.map((e) =>
          '<tr><td>' + esc(e.sequence || e.seq) + '</td>' +
          '<td>' + esc(e.eventType || e.kind) + '</td>' +
          '<td>' + global.AuroraTimestamp.render({ value: e.createdAt || e.timestamp, context: 'detail' }) + '</td>' +
          '<td>' + (e.did ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(e.did) : '<code>' + esc(e.did) + '</code>') : '—') + '</td></tr>'
        ).join('') + '</tbody></table>';
    } catch (e) {
      document.getElementById('seq-recent').innerHTML = '<h3>Recent events</h3><p class="empty-state">Unavailable.</p>';
    }
  }

  async function doAction(nsidLeaf, prompt) {
    // Per V04_DESIGN §5.3.3's Sequencer decision: sequencer ops are
    // almost universally destructive (cursor adjustments, event
    // replay, restart), so the dispatcher uses destructiveConfirm
    // uniformly rather than per-op classification. If a non-
    // destructive doAction caller appears in the future, it gets
    // its own non-destructive path.
    const result = await global.AuroraModal.destructiveConfirm({
      heading: prompt,
      body: 'Sequencer operations affect the cursor and may be irreversible. Proceed?',
      confirmLabel: 'Run',
    });
    if (!result.confirmed) return;
    try {
      await global.AuroraEndpoints.ops[nsidLeaf]();
      global.AuroraToast.success('Action complete: ' + nsidLeaf);
      await refresh();
    } catch (e) {
      global.AuroraToast.danger(nsidLeaf + ' failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsSequencer', { mount: mount });
})(window);
