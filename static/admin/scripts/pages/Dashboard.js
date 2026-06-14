// Dashboard page (route: #dashboard / #ops/dashboard).
//
// Per v09_UI_Design.md §5.2. v0.9 retires the v0.2 operator/moderator
// tab toggle (§5.2.1) for a single role-tiered landing (§5.2.2): one
// continuous page whose blocks compose by the operator's role AND the
// current moderation-mode. Each block surfaces information with
// click-through to its detail page; the Dashboard itself has no action
// affordances (§5.2.4).
//
// Block visibility (§5.2.2):
//   1 Deployment identity      any operator,   all modes
//   2 Recent activity feed      any operator,   all modes
//   3 Moderation work           Moderator+,     full only
//   4 Moderation activity       Moderator+,     full only
//   5 Deployment overview       Admin+,         full/reduced
//   6 System health summary     Admin+,         full/reduced
//   7 Kryphocron summary        Admin+,         full/reduced  (Arc D stub)
//   8 Recent admin actions      SuperAdmin,     full/reduced
//   9 Configuration posture     SuperAdmin,     full/reduced
//
// Graceful degradation (recon §4.4): a block whose backend isn't wired
// yet (Kryphocron) renders an "available later" placeholder rather than
// failing; data blocks swallow fetch errors and keep their last paint.
// Every block re-renders on the 30s poll — there are no persistent
// Chart.js instances, which retires the v0.8 "charts don't repaint"
// debt (§10.1.3) and the synthetic user-growth series by construction.
//
// i18n: operator strings here are English literals, consistent with the
// rest of the page substrate; routing the new v0.9 pages through t() is
// the dedicated A-i18n-readiness pass (#205), not this composition pass.

(function (global) {
  'use strict';

  let pollHandle = null;
  let metricsTimeRange = 'last_30d';

  function metricsGranularityFor(preset) {
    switch (preset) {
      case 'last_hour': return 'hour';
      case 'last_24h':  return 'hour';
      case 'last_7d':   return 'day';
      case 'last_30d':  return 'day';
      default:          return 'day';
    }
  }

  // --- Role/mode context resolved once per mount ---
  function ctx() {
    const s = global.AuroraSession;
    const mode = global.AuroraSettings ? global.AuroraSettings.getModerationMode() : 'full';
    return {
      isMod: s ? s.hasRole('moderator') : false,
      isAdmin: s ? s.hasRole('admin') : false,
      isSuper: s ? s.hasRole('superadmin') : false,
      mode: mode,
      full: mode === 'full',
      notDisabled: mode !== 'disabled',
    };
  }

  function esc(v) { return global.AuroraDom ? global.AuroraDom.esc(v) : String(v == null ? '' : v); }
  function icon(name, size) { return global.AuroraIcons ? global.AuroraIcons.render(name, size || 20) : ''; }
  function setText(id, v) { const el = document.getElementById(id); if (el) el.textContent = String(v); }
  function setHtml(id, html) { const el = document.getElementById(id); if (el) el.innerHTML = html; }
  function fmt() { return global.AuroraFormat; }

  // --- The block registry. Each block: visible(c) gate, html() initial
  //     markup, refresh(ep) data pull. mount() renders visible blocks
  //     and refresh() re-runs each visible block's refresh on the poll. ---
  const BLOCKS = [
    {
      id: 'identity',
      visible: () => true,
      html: () =>
        '<section class="dash-block dash-identity" id="dash-identity">' +
        '  <div class="dash-identity-main">' +
        '    <h3 id="dash-instance-name">' + esc(window.location.host) + '</h3>' +
        '    <p class="page-subtitle"><a href="' + esc(window.location.origin) + '">' + esc(window.location.origin) + '</a>' +
        '      <span id="dash-version" class="dash-version"></span></p>' +
        '  </div>' +
        '  <div class="dash-identity-session" id="dash-session"></div>' +
        '</section>',
      refresh: async (ep) => {
        const s = global.AuroraSession;
        const u = s && s.user ? s.user() : null;
        const handle = (u && u.handle) || 'operator';
        const role = (s && s.role) ? s.role() : 'operator';
        setHtml('dash-session',
          '<span class="dash-session-handle">@' + esc(handle) + '</span>' +
          '<span class="dash-session-role">' + esc(role) + '</span>');
        try {
          const v = await ep.ops.getVersionInfo();
          const ver = (v && (v.version || v.aurora_locus_version || v.serviceVersion)) || '';
          if (ver) setText('dash-version', 'Aurora-Locus ' + ver);
        } catch (e) { /* leave version blank */ }
      },
    },
    {
      id: 'recent',
      visible: () => true,
      html: () =>
        '<section class="dash-block activity-card" id="dash-recent">' +
        '  <h3>Recent activity</h3>' +
        '  <div class="activity-list" id="dash-recent-list">' +
        (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'Loading…' }) : 'Loading…') +
        '  </div>' +
        '</section>',
      refresh: async (ep, c) => {
        let rows = [];
        try {
          if (c.isMod) {
            const data = await ep.moderator.queryEvents({ limit: 20 });
            rows = (data && (data.items || data.events)) || [];
          } else {
            const data = await ep.atproto.listRecentEvents({ limit: 20 });
            rows = (data && (data.events || data.items)) || [];
          }
        } catch (e) { return; }
        const el = document.getElementById('dash-recent-list');
        if (!el) return;
        if (!rows.length) {
          el.innerHTML = global.AuroraEmptyState
            ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No recent activity' })
            : '<p class="empty-state">No recent activity.</p>';
          return;
        }
        const f = fmt();
        el.innerHTML = rows.slice(0, 20).map((e) => {
          const label = esc(e.eventType || e.kind || e.action || e.type || 'event');
          const when = f ? f.relativeTime(e.createdAt || e.timestamp || e.created_at) : '';
          const href = e.id ? '#mod/events/' + encodeURIComponent(e.id) : null;
          const inner =
            '<div class="activity-icon">' + icon('shield-alert', 18) + '</div>' +
            '<div class="activity-content">' +
            '  <div class="activity-text">' + label + '</div>' +
            '  <div class="activity-time">' + esc(when) + '</div>' +
            '</div>';
          return href
            ? '<a class="activity-item" href="' + href + '">' + inner + '</a>'
            : '<div class="activity-item">' + inner + '</div>';
        }).join('');
      },
    },
    {
      id: 'modwork',
      visible: (c) => c.isMod && c.full,
      html: () =>
        '<section class="dash-block" id="dash-modwork">' +
        '  <h3>Moderation work</h3>' +
        '  <div class="stats-grid">' +
        statCard('inbox', 'Open reports', 'dash-open-reports', '0', '#mod/reports') +
        statCard('scale', 'Pending appeals', 'dash-pending-appeals', '0', '#mod/appeals') +
        statCard('clock', 'Oldest open report', 'dash-oldest-age', '—', '#mod/queue') +
        statCard('gauge', 'Avg time to resolve', 'dash-avg-age', '—', '#mod/queue') +
        '  </div>' +
        '</section>',
      refresh: async (ep) => {
        try {
          const stats = await ep.admin.getQueueStats();
          if (!stats) return;
          const f = fmt();
          setText('dash-open-reports', stats.openReports || 0);
          setText('dash-pending-appeals', stats.pendingAppeals || 0);
          setText('dash-oldest-age', f ? f.durationCompact(stats.oldestOpenReportAgeSeconds || 0) : '—');
          setText('dash-avg-age', f ? f.durationCompact(stats.averageAgeOpenReportsSeconds || 0) : '—');
        } catch (e) { /* keep last paint */ }
      },
    },
    {
      id: 'modmetrics',
      visible: (c) => c.isMod && c.full,
      html: () =>
        '<section class="dash-block activity-card" id="dash-modmetrics">' +
        '  <div class="metrics-header">' +
        '    <h3>Moderation activity</h3>' +
        '    <label class="metrics-range-label" for="dash-metrics-range">Time range' +
        '      <select id="dash-metrics-range" class="metrics-range-select">' +
        '        <option value="last_hour">Last hour</option>' +
        '        <option value="last_24h">Last 24 hours</option>' +
        '        <option value="last_7d">Last 7 days</option>' +
        '        <option value="last_30d" selected>Last 30 days</option>' +
        '      </select>' +
        '    </label>' +
        '  </div>' +
        '  <div id="dash-metrics-body">' +
        (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'Loading…' }) : 'Loading…') +
        '  </div>' +
        '</section>',
      refresh: async (ep) => {
        wireMetricsRange();
        try {
          // The three metrics the v0.8 Dashboard proved the backend
          // emits. Appeals filed/resolved (§5.2.2) join once confirmed
          // supported — requesting an unknown metric risks a 400 that
          // would blank the whole block.
          const data = await ep.admin.getModerationMetrics({
            timeRange: metricsTimeRange,
            granularity: metricsGranularityFor(metricsTimeRange),
            metrics: ['reportsFiled', 'reportsResolved', 'actionsTaken'],
          });
          renderMetrics(data);
        } catch (e) { /* keep last paint */ }
      },
    },
    {
      id: 'overview',
      visible: (c) => c.isAdmin && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-overview">' +
        '  <h3>Deployment overview</h3>' +
        '  <div class="stats-grid">' +
        statCard('users', 'Accounts', 'dash-ov-accounts', '0', '#ops/accounts') +
        statCard('file-text', 'Records', 'dash-ov-records', '0', null) +
        statCard('image', 'Storage', 'dash-ov-storage', '0 GB', '#ops/blob-ops') +
        statCard('shield-alert', 'Open reports', 'dash-ov-reports', '0', '#mod/reports') +
        '  </div>' +
        '</section>',
      refresh: async (ep) => {
        // getInstanceMetrics is the design-named richer source (§5.2.3);
        // getStats is the proven known-shape fallback. Try the former,
        // fall back to the latter so the block always populates.
        let d = null;
        try { d = await ep.ops.getInstanceMetrics(); } catch (e) { /* fall through */ }
        if (!d || typeof d !== 'object') {
          try { d = await ep.ops.getStats(); } catch (e) { return; }
        }
        if (!d) return;
        const accounts = d.totalUsers != null ? d.totalUsers : (d.accounts != null ? d.accounts : 0);
        const records = d.totalPosts != null ? d.totalPosts : (d.records != null ? d.records : 0);
        const bytes = d.storageBytes != null ? d.storageBytes : (d.storageBytesTotal || 0);
        setText('dash-ov-accounts', accounts);
        setText('dash-ov-records', records);
        setText('dash-ov-storage', (bytes / 1073741824).toFixed(2) + ' GB');
        setText('dash-ov-reports', d.openReports || 0);
      },
    },
    {
      id: 'health',
      visible: (c) => c.isAdmin && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-health">' +
        '  <div class="metrics-header">' +
        '    <h3>System health</h3>' +
        '    <a class="btn-sm btn-secondary" href="#ops/system-health">Details</a>' +
        '  </div>' +
        '  <div id="dash-health-body"><p class="empty-state">Loading…</p></div>' +
        '</section>',
      refresh: async (ep) => {
        try {
          const h = await ep.ops.getSystemHealth();
          const overall = (h && (h.status || h.overall || h.overallStatus)) || 'unknown';
          const badge = global.AuroraStatusBadge ? global.AuroraStatusBadge.render(overall) : esc(overall);
          // Surface any subsystem map defensively (name → status).
          const subs = (h && (h.subsystems || h.components)) || null;
          let body = '<p>Overall: ' + badge + '</p>';
          if (subs && typeof subs === 'object') {
            const items = Array.isArray(subs)
              ? subs.map((s) => [s.name || s.subsystem || '?', s.status || '?'])
              : Object.keys(subs).map((k) => [k, (subs[k] && subs[k].status) || subs[k]]);
            body += '<ul class="dash-health-list">' + items.map((p) =>
              '<li>' + esc(p[0]) + ' — ' +
              (global.AuroraStatusBadge ? global.AuroraStatusBadge.render(String(p[1])) : esc(p[1])) +
              '</li>').join('') + '</ul>';
          }
          setHtml('dash-health-body', body);
        } catch (e) {
          setHtml('dash-health-body', '<p class="empty-state">Health data unavailable. <a href="#ops/system-health">Open System health</a>.</p>');
        }
      },
    },
    {
      id: 'kryphocron',
      visible: (c) => c.isAdmin && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-kryphocron">' +
        '  <h3>Kryphocron</h3>' +
        '  <div class="empty-state" role="status">' +
        '    <p>The Kryphocron summary arrives with the Kryphocron domain in 0.9.1. ' +
        '       <a href="#kryphocron">Preview</a>.</p>' +
        '  </div>' +
        '</section>',
      refresh: async () => { /* Arc D wires the summary endpoints (§6.9). */ },
    },
    {
      id: 'adminactions',
      visible: (c) => c.isSuper && c.notDisabled,
      html: () =>
        '<section class="dash-block activity-card" id="dash-adminactions">' +
        '  <div class="metrics-header">' +
        '    <h3>Recent administrative actions</h3>' +
        '    <a class="btn-sm btn-secondary" href="#mod/audit">Audit trail</a>' +
        '  </div>' +
        '  <div class="activity-list" id="dash-adminactions-list">' +
        '    <p class="empty-state">Loading…</p>' +
        '  </div>' +
        '</section>',
      refresh: async (ep) => {
        try {
          const data = await ep.admin.getAuditTrail({ limit: 10 });
          const rows = (data && (data.entries || data.items)) || [];
          const el = document.getElementById('dash-adminactions-list');
          if (!el) return;
          if (!rows.length) {
            el.innerHTML = global.AuroraEmptyState
              ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No recent administrative actions' })
              : '<p class="empty-state">None.</p>';
            return;
          }
          const f = fmt();
          el.innerHTML = rows.slice(0, 10).map((e) => {
            const label = esc(e.action || e.eventType || e.kind || 'action');
            const when = f ? f.relativeTime(e.createdAt || e.timestamp || e.created_at) : '';
            const href = e.id ? '#mod/audit/' + encodeURIComponent(e.id) : null;
            const inner =
              '<div class="activity-icon">' + icon('archive', 18) + '</div>' +
              '<div class="activity-content">' +
              '  <div class="activity-text">' + label + '</div>' +
              '  <div class="activity-time">' + esc(when) + '</div>' +
              '</div>';
            return href
              ? '<a class="activity-item" href="' + href + '">' + inner + '</a>'
              : '<div class="activity-item">' + inner + '</div>';
          }).join('');
        } catch (e) {
          setHtml('dash-adminactions-list', '<p class="empty-state">Audit data unavailable.</p>');
        }
      },
    },
    {
      id: 'drift',
      visible: (c) => c.isSuper && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-drift">' +
        '  <h3>Configuration posture</h3>' +
        '  <div id="dash-drift-body"><p class="empty-state">Loading…</p></div>' +
        '</section>',
      refresh: async (ep) => {
        // Best-effort drift signal: surface high-impact runtime settings
        // whose source tier is not the compiled Default. The composite
        // read broadens as more keys are enumerated; moderation-mode is
        // the known key with a source envelope today.
        try {
          const d = await ep.admin.getRuntimeSetting('moderation-mode');
          const value = (d && d.value) || 'full';
          const source = (d && d.source) || 'Default';
          const overridden = source && source !== 'Default';
          setHtml('dash-drift-body',
            '<p>Moderation mode: <strong>' + esc(value) + '</strong> ' +
            '<span class="dash-source-tag">(' + esc(source) + ')</span></p>' +
            '<p class="page-subtitle">' +
            (overridden
              ? 'This deployment is running a non-default value for a high-impact setting.'
              : 'High-impact settings are at their defaults.') +
            ' <a href="#configuration/general">Review configuration</a>.</p>');
        } catch (e) {
          setHtml('dash-drift-body', '<p class="empty-state">Configuration posture unavailable.</p>');
        }
      },
    },
  ];

  let activeBlocks = [];

  function mount({ container }) {
    const c = ctx();
    activeBlocks = BLOCKS.filter((b) => b.visible(c));
    container.innerHTML =
      '<header class="page-header"><div>' +
      '  <h2>Dashboard</h2>' +
      '  <p class="page-subtitle">Overview of your deployment</p>' +
      '</div></header>' +
      '<div class="dash-grid">' + activeBlocks.map((b) => b.html()).join('') + '</div>';

    refresh();
    pollHandle = setInterval(refresh, 30_000);
    return { unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    const c = ctx();
    for (const b of activeBlocks) {
      // Each block owns its errors; one failing block never blanks others.
      Promise.resolve(b.refresh(ep, c)).catch(() => {});
    }
  }

  function statCard(ic, label, valueId, initial, href) {
    const card =
      '<div class="stat-card">' +
      '  <div class="stat-icon">' + icon(ic, 28) + '</div>' +
      '  <div class="stat-content">' +
      '    <p class="stat-label">' + esc(label) + '</p>' +
      '    <p class="stat-value" id="' + valueId + '">' + esc(initial || '0') + '</p>' +
      '  </div>' +
      '</div>';
    return href ? '<a class="stat-card-link" href="' + esc(href) + '">' + card + '</a>' : card;
  }

  function wireMetricsRange() {
    const sel = document.getElementById('dash-metrics-range');
    if (!sel || sel.dataset.wired === 'true') return;
    sel.dataset.wired = 'true';
    sel.value = metricsTimeRange;
    sel.addEventListener('change', () => {
      metricsTimeRange = sel.value;
      const ep = global.AuroraEndpoints;
      const block = activeBlocks.find((b) => b.id === 'modmetrics');
      if (ep && block) Promise.resolve(block.refresh(ep, ctx())).catch(() => {});
    });
  }

  function renderMetrics(data) {
    const c = document.getElementById('dash-metrics-body');
    if (!c) return;
    if (!data || !data.series || data.series.length === 0) {
      c.innerHTML = global.AuroraEmptyState
        ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No metrics available for this range.' })
        : '<p class="empty-state">No metrics available.</p>';
      return;
    }
    let html = '<table class="data-table"><thead><tr>' +
               '<th>Metric</th><th>This period</th><th>Previous</th><th>Change</th></tr></thead><tbody>';
    for (const s of data.series) {
      const aggregate = (s.aggregate || 0).toFixed(1);
      const prev = s.delta ? (s.delta.previousAggregate || 0).toFixed(1) : '—';
      let change = '—';
      let changeClass = 'neutral';
      if (s.delta) {
        const pct = s.delta.changePercent || 0;
        const sign = pct >= 0 ? '+' : '';
        change = sign + pct.toFixed(1) + '%';
        const negSign = s.metric === 'reports_filed' ? -1 : 1;
        changeClass = pct * negSign > 0 ? 'positive' : (pct === 0 ? 'neutral' : 'attention');
      }
      html += '<tr><td>' + esc(s.metric) + '</td><td>' + aggregate + '</td><td>' + prev + '</td>' +
              '<td class="stat-change ' + changeClass + '">' + esc(change) + '</td></tr>';
    }
    html += '</tbody></table>';
    c.innerHTML = html;
  }

  if (global.AuroraRouter) global.AuroraRouter.register('dashboard', { mount: mount });
})(window);
