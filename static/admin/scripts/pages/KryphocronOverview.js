// Kryphocron Overview (§6.4.1) — the domain landing. Substrate identity,
// install-validation status, process-shape indicator, aggregate counts, a
// tier mini-summary, the rotation-status card (links to Laquna), the
// oracle-activity stub, and a recent kryphocron-class activity feed.
//
// Route: #kryphocron + #kryphocron/overview. Role: Moderator+ (read-only;
// no action affordances — actions live on per-page surfaces). Reads
// getSubstrateInfo / getTierStats / getRotationStatus / getOracleActivity
// (#225) + getAuditTrail filtered to kryphocron-class events.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);
  const K = () => global.AuroraEndpoints.ops.kryphocron;

  // kryphocron substrate audit-event types surfaced in the activity feed
  // (§6.4.1). Private-tier writes + capability binds + rewrite-on-rotate.
  const KRYPHO_EVENT_MARKERS = [
    'ContentEncoded', 'ContentEncodeFailed', 'ContentDecodeFailed',
    'MalformedRecordRejected', 'RewriteOnRotate', 'CapabilityBound',
    'kryphocron', 'tools.kryphocron',
  ];

  let pollHandle = null;

  function statusDot(outcome) {
    const ok = String(outcome || '').toLowerCase();
    const cls = ok === 'ok' ? 'status-ok' : (ok ? 'status-warn' : 'status-unknown');
    return '<span class="status-dot ' + cls + '" aria-hidden="true"></span>';
  }

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header"><h2>' + esc(t('kryphocron.overview.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('kryphocron.overview.subtitle')) + '</p></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card" id="ko-identity"><p>' + esc(t('common.loading')) + '</p></div>' +
      '  <div class="settings-card" id="ko-counts"><p>' + esc(t('common.loading')) + '</p></div>' +
      '  <div class="settings-card" id="ko-rotation"><p>' + esc(t('common.loading')) + '</p></div>' +
      '  <div class="settings-card" id="ko-oracle"><p>' + esc(t('common.loading')) + '</p></div>' +
      '  <div class="settings-card" id="ko-activity"><p>' + esc(t('common.loading')) + '</p></div>' +
      '</div>';

    await refresh();
    pollHandle = setInterval(refresh, 30000);
    return { unmount: function () { if (pollHandle) clearInterval(pollHandle); pollHandle = null; } };
  }

  async function refresh() {
    await Promise.all([
      renderIdentityAndCounts(),
      renderRotation(),
      renderOracle(),
      renderActivity(),
    ]);
  }

  async function renderIdentityAndCounts() {
    const idCard = document.getElementById('ko-identity');
    const countsCard = document.getElementById('ko-counts');
    let info = null, shape = null;
    try {
      info = await K().getSubstrateInfo();
    } catch (e) {
      if (idCard) idCard.innerHTML = errorCard(t('kryphocron.overview.identity_title'), e);
      if (countsCard) countsCard.innerHTML = '';
      return;
    }
    try {
      const s = await global.AuroraEndpoints.admin.getRuntimeSetting('kryphocron.deployment.process-shape');
      shape = s && (s.value !== undefined ? s.value : s);
    } catch (e) { shape = null; }

    const hash = String(info.lexiconRegistryHash || '');
    const hashShort = hash ? hash.slice(0, 12) + '…' : '—';
    const oracle = info.rotationOracle || '—';
    // Process-shape mismatch (§8.3.4): declared multi-process but the
    // standard single-process oracle is installed.
    const mismatch = String(shape) === 'multi-process' && oracle === 'aurora-locus-standard';

    if (idCard) {
      idCard.innerHTML =
        '<h3>' + esc(t('kryphocron.overview.identity_title')) + '</h3>' +
        '<dl class="kv-list">' +
        kv(t('kryphocron.overview.version'), esc(info.version || '—')) +
        kv(t('kryphocron.overview.registry_hash'),
           '<code title="' + esc(hash) + '">' + esc(hashShort) + '</code>') +
        kv(t('kryphocron.overview.codec'), esc(info.codecId || '—')) +
        kv(t('kryphocron.overview.oracle'), esc(oracle)) +
        kv(t('kryphocron.overview.install_validation'),
           statusDot(info.installValidation) + ' ' + esc(info.installValidation || '—')) +
        kv(t('kryphocron.overview.process_shape'), esc(shape || t('kryphocron.overview.shape_undeclared'))) +
        '</dl>' +
        (mismatch
          ? '<div class="banner banner-warning" role="alert">' +
            esc(t('kryphocron.overview.shape_mismatch')) + '</div>'
          : '');
    }
    if (countsCard) {
      const c = info.aggregateCounts || {};
      countsCard.innerHTML =
        '<h3>' + esc(t('kryphocron.overview.counts_title')) + '</h3>' +
        '<div class="stat-row">' +
        stat(t('kryphocron.overview.count_audiences'), c.audienceRecords) +
        stat(t('kryphocron.overview.count_private_posts'), c.privatePostRecords) +
        stat(t('kryphocron.overview.count_private_tier'), c.privateTierRecords) +
        stat(t('kryphocron.overview.count_public_tier'), c.publicTierRecords) +
        '</div>' +
        '<p class="settings-help"><a href="#kryphocron/tier-activity">' +
        esc(t('kryphocron.overview.see_tier_activity')) + '</a></p>';
    }
  }

  async function renderRotation() {
    const card = document.getElementById('ko-rotation');
    if (!card) return;
    try {
      const r = await K().getRotationStatus();
      card.innerHTML =
        '<h3>' + esc(t('kryphocron.overview.rotation_title')) + '</h3>' +
        '<dl class="kv-list">' +
        kv(t('kryphocron.overview.generation'), '<code>' + esc(r.generationMark || '—') + '</code>') +
        kv(t('kryphocron.overview.last_slug_rotation'), esc(fmtTime(r.lastSlugRotation))) +
        kv(t('kryphocron.overview.next_rotation'),
           esc(r.nextScheduledSlugRotation ? fmtTime(r.nextScheduledSlugRotation) : t('kryphocron.overview.next_manual'))) +
        kv(t('kryphocron.overview.rewrite_in_progress'),
           esc(r.rewriteInProgress ? t('kryphocron.overview.yes') : t('kryphocron.overview.no'))) +
        '</dl>' +
        '<p class="settings-help"><a href="#kryphocron/laquna">' +
        esc(t('kryphocron.overview.open_laquna')) + '</a></p>';
    } catch (e) {
      card.innerHTML = errorCard(t('kryphocron.overview.rotation_title'), e);
    }
  }

  async function renderOracle() {
    const card = document.getElementById('ko-oracle');
    if (!card) return;
    try {
      const o = await K().getOracleActivity();
      const body = o && o.instrumented
        ? '<p class="stat-value">' + esc(String(o.consultations != null ? o.consultations : '—')) + '</p>'
        : '<div class="empty-state" role="status"><p>' +
          esc((o && o.message) || t('kryphocron.overview.oracle_pending')) + '</p></div>';
      card.innerHTML = '<h3>' + esc(t('kryphocron.overview.oracle_title')) + '</h3>' + body;
    } catch (e) {
      card.innerHTML = errorCard(t('kryphocron.overview.oracle_title'), e);
    }
  }

  async function renderActivity() {
    const card = document.getElementById('ko-activity');
    if (!card) return;
    let entries = [];
    try {
      const out = await global.AuroraEndpoints.admin.getAuditTrail({ limit: 40 });
      entries = (out && (out.entries || out.items || out.auditTrail)) || (Array.isArray(out) ? out : []);
    } catch (e) {
      card.innerHTML = errorCard(t('kryphocron.overview.activity_title'), e);
      return;
    }
    const kryph = entries.filter(isKryphocronEvent).slice(0, 20);
    const rows = kryph.length
      ? kryph.map(activityRow).join('')
      : '<li class="empty-state"><p>' + esc(t('kryphocron.overview.activity_empty')) + '</p></li>';
    card.innerHTML =
      '<h3>' + esc(t('kryphocron.overview.activity_title')) + '</h3>' +
      '<p class="settings-help">' + esc(t('kryphocron.overview.activity_subtitle')) + '</p>' +
      '<ul class="activity-list">' + rows + '</ul>';
  }

  function isKryphocronEvent(e) {
    const blob = JSON.stringify(e || {});
    return KRYPHO_EVENT_MARKERS.some((m) => blob.indexOf(m) !== -1);
  }

  function activityRow(e) {
    const action = e.action || e.eventType || e.type || e.kind || '—';
    const subject = e.subject || e.subjectUri || e.subjectDid || '';
    const when = e.createdAt || e.at || e.indexedAt || '';
    return (
      '<li class="activity-item">' +
      '<span class="activity-action">' + esc(action) + '</span> ' +
      (subject ? '<code class="activity-subject">' + esc(subject) + '</code> ' : '') +
      '<span class="activity-time">' + esc(fmtTime(when)) + '</span>' +
      '</li>'
    );
  }

  // --- small render helpers ---
  function kv(label, valueHtml) {
    return '<div class="kv-row"><dt>' + esc(label) + '</dt><dd>' + valueHtml + '</dd></div>';
  }
  function stat(label, n) {
    return '<div class="stat"><div class="stat-value">' + esc(String(n == null ? '—' : n)) +
      '</div><div class="stat-label">' + esc(label) + '</div></div>';
  }
  function errorCard(title, e) {
    return '<h3>' + esc(title) + '</h3><div class="banner banner-error" role="alert">' +
      esc(t('common.error', { message: (e && e.message) || '' })) + '</div>';
  }
  function fmtTime(s) {
    if (!s) return '—';
    try {
      const d = new Date(s);
      if (isNaN(d.getTime())) return String(s);
      return d.toLocaleString();
    } catch (_) { return String(s); }
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('kryphocronOverview', { mount: mount });
  }
})(window);
