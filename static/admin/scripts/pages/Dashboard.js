// Dashboard page (route: #dashboard / #ops/dashboard).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.1. Two flavors share the page:
// Operator (instance metrics) and Moderator (queue depth, recent
// activity, throughput). Tab toggle switches; default Moderator if
// role+mode permit, Operator otherwise.

(function (global) {
  'use strict';

  let pollHandle = null;
  let activeFlavor = 'operator';

  // Active preset for the moderation-metrics card. v0.3's
  // getModerationMetrics added canonical `timeRange` preset accepting
  // last_hour / last_24h / last_7d / last_30d; the server still
  // accepts legacy `start`/`end` pairs via dual-shape Deserialize
  // (aurora_admin.rs:2545-2597). The UI now sends the canonical
  // shape unconditionally. Default preserves the prior 30-day window.
  //
  // No "Custom range" option ships with this preset selector — there
  // is no existing custom-window picker on the Dashboard to preserve.
  // Adding one is substantive UI work beyond Step 3 sub-3d's scope.
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

  function mount({ container }) {
    const session = global.AuroraSession;
    const isModerator = session ? session.hasRole('moderator') : false;
    activeFlavor = isModerator ? 'moderator' : 'operator';

    container.innerHTML = renderShell(isModerator);
    wireTabs(container);
    refresh();

    pollHandle = setInterval(refresh, 30_000);

    return {
      unmount: () => { if (pollHandle) clearInterval(pollHandle); pollHandle = null; },
    };
  }

  function renderShell(showTabs) {
    const tabs = showTabs ? renderTabs() : '';
    return '<header class="page-header">' +
           '  <div>' +
           '    <h2>Dashboard</h2>' +
           '    <p class="page-subtitle">Overview of your PDS instance</p>' +
           '  </div>' +
           '</header>' +
           tabs +
           '<div id="dashboard-operator">' + operatorBody() + '</div>' +
           '<div id="dashboard-moderator" style="display: ' + (showTabs ? 'none' : 'none') + ';">' +
           moderatorBody() + '</div>';
  }

  function renderTabs() {
    return '<div id="dashboard-flavor-tabs" role="tablist" aria-label="Dashboard flavor">' +
           '  <button type="button" role="tab" aria-selected="true" data-flavor="operator" class="active">Operator</button>' +
           '  <button type="button" role="tab" aria-selected="false" data-flavor="moderator">Moderator</button>' +
           '</div>';
  }

  function operatorBody() {
    return '<div class="stats-grid">' +
           statCard('users', 'Total Users', 'stat-users') +
           statCard('file-text', 'Total Posts', 'stat-posts') +
           statCard('shield-alert', 'Pending Reports', 'stat-reports') +
           statCard('image', 'Storage Used', 'stat-storage', '0 GB') +
           '</div>' +
           '<div class="charts-grid">' +
           '  <div class="chart-card"><h3>User Growth</h3><canvas id="userGrowthChart"></canvas></div>' +
           '  <div class="chart-card"><h3>Activity Overview</h3><canvas id="activityChart"></canvas></div>' +
           '</div>' +
           '<div class="activity-card">' +
           '  <h3>Recent Activity</h3>' +
           '  <div class="activity-list" id="recent-activity">' +
           (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'Loading…' }) : 'Loading…') +
           '  </div>' +
           '</div>';
  }

  function moderatorBody() {
    return '<div class="stats-grid">' +
           statCard('inbox', 'Open Reports', 'mod-stat-open-reports') +
           statCard('scale', 'Pending Appeals', 'mod-stat-pending-appeals') +
           statCard('gavel', 'Queue attention', 'mod-stat-queue-total') +
           statCard('clock', 'Oldest open report', 'mod-stat-oldest-age', '—') +
           '</div>' +
           '<div class="activity-card" style="margin-top: 1rem;">' +
           '  <div class="metrics-header">' +
           '    <h3>Moderation metrics</h3>' +
           '    <label class="metrics-range-label" for="mod-metrics-range">' +
           '      Time range' +
           '      <select id="mod-metrics-range" class="metrics-range-select">' +
           '        <option value="last_hour">Last hour</option>' +
           '        <option value="last_24h">Last 24 hours</option>' +
           '        <option value="last_7d">Last 7 days</option>' +
           '        <option value="last_30d" selected>Last 30 days</option>' +
           '      </select>' +
           '    </label>' +
           '  </div>' +
           '  <div id="mod-metrics-chart">' +
           (global.AuroraEmptyState ? global.AuroraEmptyState.render({ icon: 'loader-2', primary: 'Loading…' }) : 'Loading…') +
           '  </div>' +
           '</div>';
  }

  function statCard(icon, label, valueId, initial) {
    const ic = global.AuroraIcons ? global.AuroraIcons.render(icon, 28) : '';
    return '<div class="stat-card">' +
           '  <div class="stat-icon">' + ic + '</div>' +
           '  <div class="stat-content">' +
           '    <p class="stat-label">' + label + '</p>' +
           '    <p class="stat-value" id="' + valueId + '">' + (initial || '0') + '</p>' +
           '    <p class="stat-change neutral">—</p>' +
           '  </div>' +
           '</div>';
  }

  function wireTabs(container) {
    container.querySelectorAll('#dashboard-flavor-tabs button').forEach((btn) => {
      btn.addEventListener('click', () => {
        activeFlavor = btn.dataset.flavor;
        container.querySelectorAll('#dashboard-flavor-tabs button').forEach((b) => {
          b.classList.toggle('active', b === btn);
          b.setAttribute('aria-selected', b === btn ? 'true' : 'false');
        });
        document.getElementById('dashboard-operator').style.display = activeFlavor === 'operator' ? '' : 'none';
        document.getElementById('dashboard-moderator').style.display = activeFlavor === 'moderator' ? '' : 'none';
        refresh();
      });
    });
  }

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    // Operator pull
    try {
      const data = await ep.ops.getStats();
      setText('stat-users', data.totalUsers || 0);
      setText('stat-posts', data.totalPosts || 0);
      setText('stat-reports', data.openReports || 0);
      const storageGB = ((data.storageBytes || 0) / 1024 / 1024 / 1024).toFixed(2);
      setText('stat-storage', storageGB + ' GB');
      setStatChange(3, data.openReports > 0 ? 'Requires attention' : 'All clear',
                    data.openReports > 0 ? 'attention' : 'positive');
      drawCharts(data);
    } catch (e) {
      setText('stat-users', '0');
    }
    refreshActivity();
    if (activeFlavor === 'moderator') refreshModerator();
  }

  async function refreshActivity() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.atproto.listAccounts({ limit: 5 });
      const accounts = (data && (data.accounts || data.users)) || [];
      const items = accounts.slice(0, 3).map((u) => ({
        text: 'Account: @' + (u.handle || 'unknown'),
        time: global.AuroraFormat ? global.AuroraFormat.relativeTime(u.createdAt) : '',
        href: '#ops/accounts/' + encodeURIComponent(u.did),
      }));
      const c = document.getElementById('recent-activity');
      if (!c) return;
      if (items.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No recent activity', secondary: 'Things will appear here as accounts arrive.' })
          : '<p class="empty-state">No recent activity.</p>';
        return;
      }
      const ic = global.AuroraIcons ? global.AuroraIcons.render('users', 18) : '';
      c.innerHTML = items.map((a) =>
        '<a class="activity-item" href="' + a.href + '" style="text-decoration: none; color: inherit;">' +
        '  <div class="activity-icon">' + ic + '</div>' +
        '  <div class="activity-content">' +
        '    <div class="activity-text">' + a.text + '</div>' +
        '    <div class="activity-time">' + a.time + '</div>' +
        '  </div>' +
        '</a>'
      ).join('');
    } catch (e) { /* ignore */ }
  }

  async function refreshModerator() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    wireMetricsRangeSelect();
    try {
      const stats = await ep.admin.getQueueStats();
      if (stats) {
        setText('mod-stat-open-reports', stats.openReports || 0);
        setText('mod-stat-pending-appeals', stats.pendingAppeals || 0);
        setText('mod-stat-queue-total', stats.queueAttentionTotal || 0);
        const fmt = global.AuroraFormat;
        setText('mod-stat-oldest-age', fmt ? fmt.durationCompact(stats.oldestOpenReportAgeSeconds || 0) : '—');
      }
    } catch (e) { /* ignore */ }
    try {
      const data = await ep.admin.getModerationMetrics({
        timeRange: metricsTimeRange,
        granularity: metricsGranularityFor(metricsTimeRange),
        metrics: ['reportsFiled', 'reportsResolved', 'actionsTaken'],
      });
      renderMetrics(data);
    } catch (e) { /* ignore */ }
  }

  // Wire up the metrics time-range select. Called from refresh() to
  // pick up the select element after each render (the moderator body
  // is re-rendered when tabs switch, so the listener is idempotent).
  function wireMetricsRangeSelect() {
    const sel = document.getElementById('mod-metrics-range');
    if (!sel || sel.dataset.wired === 'true') return;
    sel.dataset.wired = 'true';
    sel.value = metricsTimeRange;
    sel.addEventListener('change', () => {
      metricsTimeRange = sel.value;
      if (activeFlavor === 'moderator') refreshModerator();
    });
  }

  function renderMetrics(data) {
    const c = document.getElementById('mod-metrics-chart');
    if (!c || !data || !data.series || data.series.length === 0) {
      if (c) c.innerHTML = global.AuroraEmptyState
        ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No metrics available for this range.' })
        : '<p class="empty-state">No metrics available.</p>';
      return;
    }
    let html = '<table class="data-table"><thead><tr>' +
               '<th>Metric</th><th>This period</th><th>Previous</th><th>Change</th></tr></thead><tbody>';
    for (const s of data.series) {
      const aggregate = (s.aggregate || 0).toFixed(1);
      const prev = s.delta ? s.delta.previousAggregate.toFixed(1) : '—';
      let change = '—';
      let changeClass = 'neutral';
      if (s.delta) {
        const pct = s.delta.changePercent;
        const sign = pct >= 0 ? '+' : '';
        change = sign + pct.toFixed(1) + '%';
        const negSign = s.metric === 'reports_filed' ? -1 : 1;
        changeClass = pct * negSign > 0 ? 'positive' : (pct === 0 ? 'neutral' : 'attention');
      }
      html += '<tr><td>' + s.metric + '</td><td>' + aggregate + '</td><td>' + prev + '</td>' +
              '<td class="stat-change ' + changeClass + '">' + change + '</td></tr>';
    }
    html += '</tbody></table>';
    c.innerHTML = html;
  }

  function drawCharts(data) {
    if (typeof Chart === 'undefined') return;
    const totalUsers = data.totalUsers || 0;
    const totalPosts = data.totalPosts || 0;
    const openReports = data.openReports || 0;
    const activeUsers = data.activeUsers || 0;
    const userCtx = document.getElementById('userGrowthChart');
    if (userCtx && !userCtx._chart) {
      const userGrowth = totalUsers > 0
        ? [0, Math.floor(totalUsers * 0.2), Math.floor(totalUsers * 0.4),
           Math.floor(totalUsers * 0.6), Math.floor(totalUsers * 0.8),
           Math.floor(totalUsers * 0.9), totalUsers]
        : [0, 0, 0, 0, 0, 0, 0];
      userCtx._chart = new Chart(userCtx, {
        type: 'line',
        data: {
          labels: ['Wk 1','Wk 2','Wk 3','Wk 4','Wk 5','Wk 6','Now'],
          datasets: [{ label: 'Total Users', data: userGrowth, borderColor: '#3b82f6',
                       backgroundColor: 'rgba(59, 130, 246, 0.1)', tension: 0.4, fill: true }],
        },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } },
                   scales: { y: { beginAtZero: true, ticks: { precision: 0 } } } },
      });
    }
    const actCtx = document.getElementById('activityChart');
    if (actCtx && !actCtx._chart) {
      actCtx._chart = new Chart(actCtx, {
        type: 'bar',
        data: {
          labels: ['Total','Active','Posts','Reports'],
          datasets: [{ label: 'Activity', data: [totalUsers, activeUsers, totalPosts, openReports],
                       backgroundColor: ['#3b82f6', '#10b981', '#f59e0b', '#ef4444'] }],
        },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } },
                   scales: { y: { beginAtZero: true, ticks: { precision: 0 } } } },
      });
    }
  }

  function setText(id, v) {
    const el = document.getElementById(id);
    if (el) el.textContent = String(v);
  }

  function setStatChange(cardIdx, text, semantic) {
    const el = document.querySelector('#dashboard-operator .stats-grid .stat-card:nth-child(' + cardIdx + ') .stat-change');
    if (!el) return;
    el.textContent = text;
    el.classList.remove('positive', 'attention', 'neutral');
    el.classList.add(semantic || 'neutral');
  }

  if (global.AuroraRouter) global.AuroraRouter.register('dashboard', { mount: mount });
})(window);
