// Laquna rotation-history sub-page (§6.4.2.1). Route
// #kryphocron/laquna/history, Admin+. Two-track side-by-side rendering of
// listRotations: operator-triggered rewrites (full metadata, from #224's
// rewrite-history.log) and cadence-organic slug rotations (compact, from
// rotation-history.log). The backend sorts equal-`at` operator-triggered
// first (§13.2); this page preserves the returned order.
//
// The cadence-organic track is empty until #238 (the oracle write-side for
// rotation-history.log) lands — the column renders an honest empty-state
// rather than implying no rotations have occurred.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);

  // §10.4.3 — canonical timestamp rendering (returns a <time> element).
  function ts(value, context) {
    return global.AuroraTimestamp.render({ value: value, context: context });
  }

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#kryphocron/overview">' + esc(t('kryphocron.overview.title')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' +
      '  <a href="#kryphocron/laquna">' + esc(t('kryphocron.laquna.title')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('kryphocron.history.title')) +
      '</nav>' +
      '<header class="page-header"><h2>' + esc(t('kryphocron.history.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('kryphocron.history.subtitle')) + '</p></header>' +
      '<div class="two-col-grid" id="lh-grid">' +
      '  <div class="settings-card"><p>' + esc(t('common.loading')) + '</p></div>' +
      '</div>';

    let rotations = [];
    try {
      const out = await global.AuroraEndpoints.ops.kryphocron.listRotations();
      rotations = (out && out.rotations) || [];
    } catch (e) {
      const grid = container.querySelector('#lh-grid');
      if (grid) grid.innerHTML = '<div class="settings-card"><div class="banner banner-error" ' +
        'role="alert">' + esc(t('common.error', { message: (e && e.message) || '' })) + '</div></div>';
      return {};
    }

    const operator = rotations.filter((r) => r.kind === 'operator-triggered');
    const organic = rotations.filter((r) => r.kind === 'cadence-organic');

    const grid = container.querySelector('#lh-grid');
    if (grid) {
      grid.innerHTML =
        '<div class="settings-card">' +
        '  <h3>' + esc(t('kryphocron.history.operator_title')) + '</h3>' +
        '  <p class="settings-help">' + esc(t('kryphocron.history.operator_subtitle')) + '</p>' +
        (operator.length ? operatorTable(operator)
          : '<div class="empty-state" role="status"><p>' +
            esc(t('kryphocron.history.operator_empty')) + '</p></div>') +
        '</div>' +
        '<div class="settings-card">' +
        '  <h3>' + esc(t('kryphocron.history.organic_title')) + '</h3>' +
        '  <p class="settings-help">' + esc(t('kryphocron.history.organic_subtitle')) + '</p>' +
        (organic.length ? organicList(organic)
          : '<div class="empty-state" role="status"><p>' +
            esc(t('kryphocron.history.organic_pending')) + '</p></div>') +
        '</div>';
    }
    return {};
  }

  function operatorTable(rows) {
    const body = rows.map((r) =>
      '<tr>' +
      '<td>' + ts(r.at, 'detail') + '</td>' +
      '<td><code>' + esc(r.generationMark || '—') + '</code></td>' +
      '<td>' + esc(String(r.recordsRewritten != null ? r.recordsRewritten : '—')) + '</td>' +
      '<td>' + esc(r.outcome || '—') + '</td>' +
      '<td>' + esc(r.durationMs != null ? (Math.round(r.durationMs / 100) / 10) + 's' : '—') + '</td>' +
      '</tr>').join('');
    return '<table class="data-table"><thead><tr>' +
      '<th>' + esc(t('kryphocron.history.col_at')) + '</th>' +
      '<th>' + esc(t('kryphocron.history.col_generation')) + '</th>' +
      '<th>' + esc(t('kryphocron.history.col_rewritten')) + '</th>' +
      '<th>' + esc(t('kryphocron.history.col_outcome')) + '</th>' +
      '<th>' + esc(t('kryphocron.history.col_duration')) + '</th>' +
      '</tr></thead><tbody>' + body + '</tbody></table>';
  }

  function organicList(rows) {
    return '<ul class="activity-list">' + rows.map((r) =>
      '<li class="activity-item"><span class="activity-time">' + ts(r.at, 'activity') +
      '</span> <code class="activity-subject">' + esc(r.generationMark || '—') + '</code></li>'
    ).join('') + '</ul>';
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('kryphocronLaqunaHistory', { mount: mount });
  }
})(window);
