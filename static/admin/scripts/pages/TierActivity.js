// Kryphocron Tier activity (§6.4.4) — record write counts by tier + NSID,
// with 30-day trends and per-account distribution. Route
// #kryphocron/tier-activity, Moderator+, read-only. Two tiers only —
// Public / Private (no whisper tier, §4.5). Reads getTierStats (#225). No
// record content is rendered — counts + metadata only.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);

  let pollHandle = null;

  // Inline sparkline of a daily time series (CSS-height bars; no chart dep).
  function sparkline(series) {
    if (!series || !series.length) return '<span class="spark-empty">—</span>';
    const max = Math.max.apply(null, series.map((p) => p.count || 0).concat([1]));
    const bars = series.map((p) => {
      const h = Math.max(2, Math.round(((p.count || 0) / max) * 24));
      return '<span class="spark-bar" style="height:' + h + 'px" title="' +
        esc(p.date + ': ' + (p.count || 0)) + '"></span>';
    }).join('');
    return '<span class="sparkline">' + bars + '</span>';
  }

  function distribution(buckets) {
    if (!buckets || !buckets.length) return '—';
    return buckets
      .filter((b) => (b.accounts || 0) > 0)
      .map((b) => esc(b.bucket) + ':' + esc(String(b.accounts)))
      .join('  ') || '—';
  }

  function nsidShort(nsid) {
    // Drop the tools.kryphocron. prefix for display density.
    return String(nsid || '').replace(/^tools\.kryphocron\./, '');
  }

  function tierBlock(titleKey, tier, nsids) {
    const rows = nsids.filter((n) => n.tier === tier);
    if (!rows.length) {
      return '<div class="settings-card"><h3>' + esc(t(titleKey)) + '</h3>' +
        '<p class="settings-help">—</p></div>';
    }
    const body = rows.map((n) =>
      '<tr>' +
      '<td><code>' + esc(nsidShort(n.nsid)) + '</code></td>' +
      '<td class="num">' + esc(String(n.total || 0)) + '</td>' +
      '<td>' + sparkline(n.timeSeries) + '</td>' +
      '<td class="dist">' + distribution(n.accountDistribution) + '</td>' +
      '</tr>').join('');
    const total = rows.reduce((s, n) => s + (n.total || 0), 0);
    return (
      '<div class="settings-card">' +
      '<h3>' + esc(t(titleKey)) + ' <span class="tier-total">(' + esc(String(total)) + ')</span></h3>' +
      '<table class="data-table"><thead><tr>' +
      '<th>' + esc(t('kryphocron.tier-activity.col_nsid')) + '</th>' +
      '<th class="num">' + esc(t('kryphocron.tier-activity.col_total')) + '</th>' +
      '<th>' + esc(t('kryphocron.tier-activity.col_trend')) + '</th>' +
      '<th>' + esc(t('kryphocron.tier-activity.col_distribution')) + '</th>' +
      '</tr></thead><tbody>' + body + '</tbody></table>' +
      '</div>'
    );
  }

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header"><h2>' + esc(t('kryphocron.tier-activity.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('kryphocron.tier-activity.subtitle')) + '</p></header>' +
      '<div class="settings-grid" id="kt-grid">' +
      '  <div class="settings-card"><p>' + esc(t('common.loading')) + '</p></div>' +
      '</div>';
    await refresh();
    pollHandle = setInterval(refresh, 30000);
    return { unmount: function () { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const grid = document.getElementById('kt-grid');
    if (!grid) return;
    let s;
    try { s = await global.AuroraEndpoints.ops.kryphocron.getTierStats(); }
    catch (e) {
      grid.innerHTML = '<div class="settings-card" id="kt-err"></div>';
      global.AuroraInlineError.mount(document.getElementById('kt-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: refresh,
      });
      return;
    }
    const nsids = (s && s.nsids) || [];
    const totals = (s && s.tierTotals) || { public: 0, private: 0 };
    if (!nsids.length) {
      grid.innerHTML = '<div class="settings-card"><div class="empty-state" role="status"><p>' +
        esc(t('kryphocron.tier-activity.empty')) + '</p></div></div>';
      return;
    }
    grid.innerHTML =
      '<div class="settings-card">' +
      '  <div class="stat-row">' +
      '    <div class="stat"><div class="stat-value">' + esc(String(totals.public || 0)) +
           '</div><div class="stat-label">' + esc(t('kryphocron.tier-activity.tier_public')) + '</div></div>' +
      '    <div class="stat"><div class="stat-value">' + esc(String(totals.private || 0)) +
           '</div><div class="stat-label">' + esc(t('kryphocron.tier-activity.tier_private')) + '</div></div>' +
      '  </div>' +
      '  <p class="settings-help">' + esc(t('kryphocron.tier-activity.window_note',
           { days: (s && s.windowDays) || 30 })) + '</p>' +
      '</div>' +
      tierBlock('kryphocron.tier-activity.public_block', 'public', nsids) +
      tierBlock('kryphocron.tier-activity.private_block', 'private', nsids);
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('kryphocronTierActivity', { mount: mount });
  }
})(window);
