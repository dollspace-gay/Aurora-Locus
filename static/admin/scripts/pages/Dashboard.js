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
// i18n (§10.3.2): operator-facing strings route through t() against the
// dashboard.* keys in i18n/en.json. English ships now; future locales are
// a routing exercise, not a per-string retrofit.

(function (global) {
  'use strict';

  let pollHandle = null;
  let metricsTimeRange = 'last_30d';
  // #361 account-growth block state. growthMode is intentionally NOT persisted
  // across loads (design pin): mount() resets it to per-day. growthData caches
  // the last fetch so the toggle re-renders without a round-trip.
  let growthMode = 'perDay';
  let growthData = null;

  function T(key, params) { return global.t ? global.t(key, params) : key; }

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
  function loadingState() {
    return global.AuroraSkeleton.lines(3);
  }

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
          if (ver) setText('dash-version', T('dashboard.version_prefix', { version: ver }));
        } catch (e) { /* leave version blank */ }
      },
    },
    {
      id: 'recent',
      visible: () => true,
      html: () =>
        '<section class="dash-block activity-card" id="dash-recent">' +
        '  <h3>' + esc(T('dashboard.recent_activity')) + '</h3>' +
        '  <div class="activity-list" id="dash-recent-list">' + loadingState() + '</div>' +
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
            ? global.AuroraEmptyState.render({ icon: 'inbox', primary: T('dashboard.no_recent_activity') })
            : '<p class="empty-state">' + esc(T('dashboard.no_recent_activity')) + '</p>';
          return;
        }
        const f = fmt();
        el.innerHTML = rows.slice(0, 20).map((e) => {
          const label = esc(e.eventType || e.kind || e.action || e.type || 'event');
          const when = global.AuroraTimestamp.render({ value: e.createdAt || e.timestamp || e.created_at, context: 'activity' });
          const href = e.id ? '#mod/events/' + encodeURIComponent(e.id) : null;
          const inner =
            '<div class="activity-icon">' + icon('shield-alert', 18) + '</div>' +
            '<div class="activity-content">' +
            '  <div class="activity-text">' + label + '</div>' +
            '  <div class="activity-time">' + when + '</div>' +
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
        '  <h3>' + esc(T('dashboard.modwork_title')) + '</h3>' +
        '  <div class="stats-grid">' +
        statCard('inbox', T('dashboard.stat_open_reports'), 'dash-open-reports', '0', '#mod/reports') +
        statCard('scale', T('dashboard.stat_pending_appeals'), 'dash-pending-appeals', '0', '#mod/appeals') +
        statCard('clock', T('dashboard.stat_oldest_report'), 'dash-oldest-age', '—', '#mod/queue') +
        statCard('gauge', T('dashboard.stat_avg_resolve'), 'dash-avg-age', '—', '#mod/queue') +
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
        '    <h3>' + esc(T('dashboard.modactivity_title')) + '</h3>' +
        '    <label class="metrics-range-label" for="dash-metrics-range">' + esc(T('dashboard.time_range')) +
        '      <select id="dash-metrics-range" class="metrics-range-select">' +
        '        <option value="last_hour">' + esc(T('dashboard.range_last_hour')) + '</option>' +
        '        <option value="last_24h">' + esc(T('dashboard.range_last_24h')) + '</option>' +
        '        <option value="last_7d">' + esc(T('dashboard.range_last_7d')) + '</option>' +
        '        <option value="last_30d" selected>' + esc(T('dashboard.range_last_30d')) + '</option>' +
        '      </select>' +
        '    </label>' +
        '  </div>' +
        '  <div id="dash-metrics-body">' + loadingState() + '</div>' +
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
        '  <h3>' + esc(T('dashboard.overview_title')) + '</h3>' +
        '  <div class="stats-grid">' +
        statCard('users', T('dashboard.stat_accounts'), 'dash-ov-accounts', '0', '#ops/accounts') +
        statCard('file-text', T('dashboard.stat_records'), 'dash-ov-records', '0', null) +
        statCard('image', T('dashboard.stat_storage'), 'dash-ov-storage', '0 GB', '#ops/blob-ops') +
        statCard('shield-alert', T('dashboard.stat_open_reports'), 'dash-ov-reports', '0', '#mod/reports') +
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
      // #361 — real account-growth visual off actor.created_at. A fixed
      // 30-day, per-day window; the header toggle picks whether the sparkline
      // shows new-accounts-per-day (default) or the cumulative deployment
      // total. One fetch serves both modes (each point carries both fields);
      // toggling re-renders from the cached series with no re-fetch.
      id: 'accountgrowth',
      visible: (c) => c.isAdmin && c.notDisabled,
      html: () =>
        '<section class="dash-block activity-card" id="dash-accountgrowth">' +
        '  <div class="metrics-header">' +
        '    <h3>' + esc(T('dashboard.growth_title')) + '</h3>' +
        '    <label class="metrics-range-label" for="dash-growth-mode">' + esc(T('dashboard.growth_mode')) +
        '      <select id="dash-growth-mode" class="metrics-range-select">' +
        '        <option value="perDay" selected>' + esc(T('dashboard.growth_per_day')) + '</option>' +
        '        <option value="cumulative">' + esc(T('dashboard.growth_cumulative')) + '</option>' +
        '      </select>' +
        '    </label>' +
        '  </div>' +
        '  <div id="dash-growth-body">' + loadingState() + '</div>' +
        '</section>',
      refresh: async (ep) => {
        wireGrowthMode();
        try {
          growthData = await ep.admin.getAccountGrowth();
        } catch (e) { return; /* keep last paint */ }
        renderGrowth();
      },
    },
    {
      id: 'health',
      visible: (c) => c.isAdmin && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-health">' +
        '  <div class="metrics-header">' +
        '    <h3>' + esc(T('dashboard.health_title')) + '</h3>' +
        '    <a class="btn-sm btn-secondary" href="#ops/system-health">' + esc(T('dashboard.health_details')) + '</a>' +
        '  </div>' +
        '  <div id="dash-health-body">' + global.AuroraSkeleton.lines(3) + '</div>' +
        '</section>',
      refresh: async (ep) => {
        try {
          const h = await ep.ops.getSystemHealth();
          const overall = (h && (h.status || h.overall || h.overallStatus)) || 'unknown';
          const badge = global.AuroraStatusBadge ? global.AuroraStatusBadge.render(overall) : esc(overall);
          const subs = (h && (h.subsystems || h.components)) || null;
          let body = '<p>' + esc(T('dashboard.health_overall')) + ': ' + badge + '</p>';
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
          setHtml('dash-health-body', '<p class="empty-state">' + esc(T('dashboard.health_unavailable')) +
            ' <a href="#ops/system-health">' + esc(T('dashboard.health_open')) + '</a></p>');
        }
      },
    },
    {
      id: 'kryphocron',
      visible: (c) => c.isAdmin && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-kryphocron">' +
        '  <div class="metrics-header">' +
        '    <h3>' + esc(T('kryphocron.dashboard-block.title')) + '</h3>' +
        '    <a class="btn-sm btn-secondary" href="#kryphocron">' +
             esc(T('kryphocron.dashboard-block.open')) + '</a>' +
        '  </div>' +
        '  <div id="dash-kryphocron-body">' + global.AuroraSkeleton.lines(3) + '</div>' +
        '</section>',
      // §6.9 — slice of the Overview's substrate identity + aggregate counts,
      // with click-through to the Kryphocron domain.
      refresh: async (ep) => {
        const body = document.getElementById('dash-kryphocron-body');
        if (!body) return;
        const stat = (n, labelKey) =>
          '<div class="stat"><div class="stat-value">' + esc(String(n == null ? '—' : n)) +
          '</div><div class="stat-label">' + esc(T(labelKey)) + '</div></div>';
        try {
          const K = ep.ops.kryphocron;
          const [info, tiers] = await Promise.all([
            K.getSubstrateInfo(),
            K.getTierStats().catch(() => null),
          ]);
          const counts = (info && info.aggregateCounts) || {};
          const tot = (tiers && tiers.tierTotals) || {};
          body.innerHTML =
            '<div class="stat-row">' +
            stat(counts.privatePostRecords, 'kryphocron.dashboard-block.private_posts') +
            stat(counts.audienceRecords, 'kryphocron.dashboard-block.audiences') +
            stat(tot.public, 'kryphocron.dashboard-block.public_tier') +
            '</div>' +
            '<p class="settings-help">' +
            esc(T('kryphocron.dashboard-block.codec', { codec: (info && info.codecId) || '—' })) + '</p>';
        } catch (e) {
          body.innerHTML = '<p class="empty-state">' +
            esc(T('kryphocron.dashboard-block.unavailable')) +
            ' <a href="#kryphocron">' + esc(T('kryphocron.dashboard-block.open')) + '</a></p>';
        }
      },
    },
    {
      id: 'adminactions',
      visible: (c) => c.isSuper && c.notDisabled,
      html: () =>
        '<section class="dash-block activity-card" id="dash-adminactions">' +
        '  <div class="metrics-header">' +
        '    <h3>' + esc(T('dashboard.adminactions_title')) + '</h3>' +
        '    <a class="btn-sm btn-secondary" href="#mod/audit">' + esc(T('dashboard.audit_trail')) + '</a>' +
        '  </div>' +
        '  <div class="activity-list" id="dash-adminactions-list">' +
        global.AuroraSkeleton.lines(3) +
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
              ? global.AuroraEmptyState.render({ icon: 'inbox', primary: T('dashboard.no_admin_actions') })
              : '<p class="empty-state">' + esc(T('dashboard.no_admin_actions')) + '</p>';
            return;
          }
          const f = fmt();
          el.innerHTML = rows.slice(0, 10).map((e) => {
            const label = esc(e.action || e.eventType || e.kind || 'action');
            const when = global.AuroraTimestamp.render({ value: e.createdAt || e.timestamp || e.created_at, context: 'activity' });
            const href = e.id ? '#mod/audit/' + encodeURIComponent(e.id) : null;
            const inner =
              '<div class="activity-icon">' + icon('archive', 18) + '</div>' +
              '<div class="activity-content">' +
              '  <div class="activity-text">' + label + '</div>' +
              '  <div class="activity-time">' + when + '</div>' +
              '</div>';
            return href
              ? '<a class="activity-item" href="' + href + '">' + inner + '</a>'
              : '<div class="activity-item">' + inner + '</div>';
          }).join('');
        } catch (e) {
          setHtml('dash-adminactions-list', '<p class="empty-state">' + esc(T('dashboard.audit_unavailable')) + '</p>');
        }
      },
    },
    {
      id: 'drift',
      visible: (c) => c.isSuper && c.notDisabled,
      html: () =>
        '<section class="dash-block" id="dash-drift">' +
        '  <h3>' + esc(T('dashboard.drift_title')) + '</h3>' +
        '  <div id="dash-drift-body">' + global.AuroraSkeleton.lines(3) + '</div>' +
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
            '<p>' + esc(T('dashboard.drift_mode', { value: value })) + ' ' +
            '<span class="dash-source-tag">' + esc(T('dashboard.drift_source', { source: source })) + '</span></p>' +
            '<p class="page-subtitle">' +
            esc(overridden ? T('dashboard.drift_overridden') : T('dashboard.drift_default')) +
            ' <a href="#configuration/general">' + esc(T('dashboard.review_config')) + '</a></p>');
        } catch (e) {
          setHtml('dash-drift-body', '<p class="empty-state">' + esc(T('dashboard.posture_unavailable')) + '</p>');
        }
      },
    },
  ];

  let activeBlocks = [];

  function mount({ container }) {
    const c = ctx();
    // Reset the account-growth toggle to its default each load (no persistence).
    growthMode = 'perDay';
    growthData = null;
    activeBlocks = BLOCKS.filter((b) => b.visible(c));
    container.innerHTML =
      '<header class="page-header"><div>' +
      '  <h2>' + esc(T('dashboard.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(T('dashboard.subtitle')) + '</p>' +
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
        ? global.AuroraEmptyState.render({ icon: 'inbox', primary: T('dashboard.metrics_none') })
        : '<p class="empty-state">' + esc(T('dashboard.metrics_none')) + '</p>';
      return;
    }
    let html = '<table class="data-table"><thead><tr>' +
               '<th>' + esc(T('dashboard.metric_col_metric')) + '</th>' +
               '<th>' + esc(T('dashboard.metric_col_period')) + '</th>' +
               '<th>' + esc(T('dashboard.metric_col_previous')) + '</th>' +
               '<th>' + esc(T('dashboard.metric_col_change')) + '</th></tr></thead><tbody>';
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

  function wireGrowthMode() {
    const sel = document.getElementById('dash-growth-mode');
    if (!sel || sel.dataset.wired === 'true') return;
    sel.dataset.wired = 'true';
    sel.value = growthMode;
    sel.addEventListener('change', () => {
      growthMode = sel.value;
      renderGrowth(); // re-render from the cached series — no re-fetch
    });
  }

  // Renders the account-growth sparkline for the current toggle mode from the
  // cached series. Reuses the TierActivity CSS-bar idiom (no chart dependency).
  function renderGrowth() {
    const c = document.getElementById('dash-growth-body');
    if (!c) return;
    const points = (growthData && Array.isArray(growthData.points)) ? growthData.points : [];
    if (!points.length) {
      c.innerHTML = global.AuroraEmptyState
        ? global.AuroraEmptyState.render({ icon: 'inbox', primary: T('dashboard.growth_none') })
        : '<p class="empty-state">' + esc(T('dashboard.growth_none')) + '</p>';
      return;
    }
    const cumulative = growthMode === 'cumulative';
    const field = cumulative ? 'cumulativeAccounts' : 'newAccounts';
    const values = points.map((p) => (p[field] || 0));
    const max = Math.max.apply(null, values.concat([1]));
    const bars = points.map((p) => {
      const v = p[field] || 0;
      const h = Math.max(2, Math.round((v / max) * 48));
      return '<span class="spark-bar" style="height:' + h + 'px" title="' +
        esc(p.day + ': ' + v) + '"></span>';
    }).join('');
    // Headline: deployment size (cumulative) or new-in-window total (per-day).
    let headline;
    if (cumulative) {
      const last = points[points.length - 1];
      headline = T('dashboard.growth_total_accounts', { count: (last && last.cumulativeAccounts) || 0 });
    } else {
      const sum = values.reduce((s, v) => s + v, 0);
      headline = T('dashboard.growth_new_in_window', { count: sum });
    }
    c.innerHTML =
      '<p class="dash-growth-headline stat-value">' + esc(headline) + '</p>' +
      '<div class="sparkline sparkline-tall">' + bars + '</div>' +
      '<p class="page-subtitle dash-growth-range">' +
        esc(T('dashboard.growth_window', {
          start: (growthData && growthData.windowStart) || '',
          end: (growthData && growthData.windowEnd) || '',
        })) + '</p>';
  }

  if (global.AuroraRouter) global.AuroraRouter.register('dashboard', { mount: mount });
})(window);
