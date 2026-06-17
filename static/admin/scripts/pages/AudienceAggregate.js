// Kryphocron Audience aggregate (§6.4.3) — deployment-wide policy.audience
// statistics. Route #kryphocron/audiences, Moderator+, read-only (operator-
// aggregate; NO per-account CRUD — account holders manage their own
// audiences via their client). Reads getAudienceAggregate (#225): totals,
// 5-mode distribution, and a list-mode member-size histogram.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);

  const MODES = ['list', 'everyone', 'followers', 'following', 'nobody', 'unset'];

  let pollHandle = null;

  // A labelled proportional bar (no chart dependency — CSS width %).
  function bar(label, value, max) {
    const pct = max > 0 ? Math.round((value / max) * 100) : 0;
    return (
      '<div class="bar-row">' +
      '<span class="bar-label">' + esc(label) + '</span>' +
      '<span class="bar-track"><span class="bar-fill" style="width:' + pct + '%"></span></span>' +
      '<span class="bar-value">' + esc(String(value)) + '</span>' +
      '</div>'
    );
  }

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header"><h2>' + esc(t('kryphocron.audiences.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('kryphocron.audiences.subtitle')) + '</p></header>' +
      '<div class="settings-grid" id="ka-grid">' +
      '  <div class="settings-card"><p>' + esc(t('common.loading')) + '</p></div>' +
      '</div>';
    await refresh();
    pollHandle = setInterval(refresh, 30000);
    return { unmount: function () { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    const grid = document.getElementById('ka-grid');
    if (!grid) return;
    let a;
    try { a = await global.AuroraEndpoints.ops.kryphocron.getAudienceAggregate(); }
    catch (e) {
      grid.innerHTML = '<div class="settings-card" id="ka-err"></div>';
      global.AuroraInlineError.mount(document.getElementById('ka-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: refresh,
      });
      return;
    }

    if (!a || !a.totalAudienceRecords) {
      grid.innerHTML = '<div class="settings-card"><div class="empty-state" role="status"><p>' +
        esc(t('kryphocron.audiences.empty')) + '</p></div></div>';
      return;
    }

    const md = a.modeDistribution || {};
    const modeMax = Math.max.apply(null, MODES.map((m) => md[m] || 0).concat([1]));
    const modeBars = MODES.map((m) => bar(t('kryphocron.audiences.mode_' + m), md[m] || 0, modeMax)).join('');

    const hist = a.listSizeHistogram || [];
    const histMax = Math.max.apply(null, hist.map((h) => h.accounts || 0).concat([1]));
    const histBars = hist.length
      ? hist.map((h) => bar(h.bucket, h.accounts || 0, histMax)).join('')
      : '<p class="settings-help">' + esc(t('kryphocron.audiences.no_list_audiences')) + '</p>';

    const avg = typeof a.averageAudiencesPerAccount === 'number'
      ? a.averageAudiencesPerAccount.toFixed(2) : '—';

    grid.innerHTML =
      '<div class="settings-card">' +
      '  <h3>' + esc(t('kryphocron.audiences.counts_title')) + '</h3>' +
      '  <div class="stat-row">' +
      '    <div class="stat"><div class="stat-value">' + esc(String(a.totalAudienceRecords)) +
           '</div><div class="stat-label">' + esc(t('kryphocron.audiences.total')) + '</div></div>' +
      '    <div class="stat"><div class="stat-value">' + esc(String(a.accountsWithAudiences || 0)) +
           '</div><div class="stat-label">' + esc(t('kryphocron.audiences.accounts_with')) + '</div></div>' +
      '    <div class="stat"><div class="stat-value">' + esc(avg) +
           '</div><div class="stat-label">' + esc(t('kryphocron.audiences.avg')) + '</div></div>' +
      '  </div>' +
      '</div>' +
      '<div class="settings-card">' +
      '  <h3>' + esc(t('kryphocron.audiences.mode_dist_title')) + '</h3>' +
      '  <div class="bar-chart">' + modeBars + '</div>' +
      '</div>' +
      '<div class="settings-card">' +
      '  <h3>' + esc(t('kryphocron.audiences.size_hist_title')) + '</h3>' +
      '  <p class="settings-help">' + esc(t('kryphocron.audiences.size_hist_help')) + '</p>' +
      '  <div class="bar-chart">' + histBars + '</div>' +
      '</div>';
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('kryphocronAudiences', { mount: mount });
  }
})(window);
