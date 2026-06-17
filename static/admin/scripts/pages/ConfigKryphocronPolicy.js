// Configuration → Kryphocron policy (§6.6). Replaces the Arc A
// configKryphocronPolicy stub. SuperAdmin-gated (route `requires:
// 'superadmin'`); each live card saves a `runtime_settings` key with a
// rationale (audit-chained), mirroring the ConfigThemes set-default flow.
//
// Live cards (backends exist today): New-account access (immediate /
// delayed), Default audience mode, Rotation cadence policy (deployment
// cadence + per-account range), Process-shape declaration. Read-only:
// the active at-rest codec (from getSubstrateInfo — Laquna is the
// substrate's default friction-encoding ContentCodec, surfaced read-only
// per §4-add.1). Backend-prereq cards (per-tier rate limits, per-account
// overrides, earned-access) render the §5.5.4 "available when X ships"
// placeholder rather than disabled inputs (§6.6.2 / §7).
//
// What this page is NOT (§6.6.1): no master kryphocron enable/disable
// (encoding-at-default is structural) and no per-tier toggles (the tier
// split is closed-namespace via KRYPHOCRON_LEXICON_REGISTRY).

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);

  // Runtime-settings keys this page owns (§6.6.2 / §8.3.4). Values stored
  // under kryphocron.policy.* + the shared laquna cadence keys the oracle
  // consumes + the deployment process-shape declaration.
  const KEY_ACCESS = 'kryphocron.policy.new-account-access'; // immediate | delayed | earned
  const KEY_ACCESS_DELAY = 'kryphocron.policy.access-delay-days'; // integer (delayed)
  const KEY_DEFAULT_AUDIENCE = 'kryphocron.policy.default-audience-mode'; // list|everyone|followers|following|nobody
  const KEY_CADENCE = 'kryphocron.laquna.rotation-cadence'; // hourly|daily|weekly|manual-only
  const KEY_CADENCE_RANGE = 'kryphocron.laquna.account-cadence-range'; // weekly-to-daily|weekly-to-hourly|no-override
  const KEY_PROCESS_SHAPE = 'kryphocron.deployment.process-shape'; // single-process|multi-process

  function selectHtml(id, options, current) {
    const opts = options
      .map(
        (o) =>
          '<option value="' + esc(o.value) + '"' +
          (o.value === current ? ' selected' : '') +
          (o.disabled ? ' disabled' : '') +
          '>' + esc(o.label) + '</option>',
      )
      .join('');
    return '<select id="' + esc(id) + '" class="form-select">' + opts + '</select>';
  }

  // A live settings card: a labelled <select> + a Save button that saves
  // `key` with a rationale.
  function liveCard(opts) {
    return (
      '<div class="settings-card" data-card="' + esc(opts.key) + '">' +
      '  <h3>' + esc(opts.title) +
          (opts.source ? ' ' + global.AuroraSourceTier.badge(opts.source) : '') + '</h3>' +
      '  <p class="settings-help">' + esc(opts.help) + '</p>' +
      '  <div class="form-row">' +
      '    <label for="' + esc(opts.selectId) + '">' + esc(opts.label) + '</label>' +
      '    ' + selectHtml(opts.selectId, opts.options, opts.current) +
      '  </div>' +
      (opts.extraHtml || '') +
      '  <div class="form-actions">' +
      '    <button class="btn btn-primary" data-save="' + esc(opts.key) + '" data-select="' +
            esc(opts.selectId) + '">' + esc(t('common.save')) + '</button>' +
      '  </div>' +
      '</div>'
    );
  }

  function prereqCard(title, blurb) {
    return (
      '<div class="settings-card">' +
      '  <h3>' + esc(title) + '</h3>' +
      '  <div class="empty-state" role="status"><p>' + esc(blurb) + '</p></div>' +
      '</div>'
    );
  }

  // Resolve a runtime setting to its { value, source } pair (§8.2.1 — the
  // source tier drives the per-card indicator), tolerating the {value, source}
  // shape and a bare value; falls back to `dflt` at the Default tier.
  async function readSetting(key, dflt) {
    try {
      const out = await global.AuroraEndpoints.admin.getRuntimeSetting(key);
      const v = out && (out.value !== undefined ? out.value : out);
      const source = out && out.source;
      if (v === undefined || v === null || v === '') {
        return { value: dflt, source: source || 'Default' };
      }
      return { value: String(v), source: source || 'Runtime' };
    } catch (e) {
      return { value: dflt, source: 'Default' };
    }
  }

  async function mount({ container }) {
    const session = global.AuroraSession;
    if (session && !session.hasRole('superadmin')) {
      container.innerHTML =
        '<header class="page-header"><h2>' + esc(t('kryphocron.policy.title')) + '</h2></header>' +
        '<div class="empty-state" role="status"><p>' +
        esc(t('errors.permissionDenied')) + '</p></div>';
      return {};
    }

    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#configuration/general">' + esc(t('settings.title')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('kryphocron.policy.title')) +
      '</nav>' +
      '<header class="page-header"><h2>' + esc(t('kryphocron.policy.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('kryphocron.policy.subtitle')) + '</p></header>' +
      '<div class="settings-grid" id="kpolicy-grid">' +
      global.AuroraSkeleton.cards(6) +
      '</div>';

    await load(container);
    const grid = container.querySelector('#kpolicy-grid');
    if (grid) {
      grid.addEventListener('click', function (ev) {
        const btn = ev.target.closest('button[data-save]');
        if (btn) save(btn.getAttribute('data-save'), btn.getAttribute('data-select'), container);
      });
    }
    return {};
  }

  async function load(container) {
    const [access, delay, defAud, cadence, range, shape, substrate] = await Promise.all([
      readSetting(KEY_ACCESS, 'immediate'),
      readSetting(KEY_ACCESS_DELAY, '7'),
      readSetting(KEY_DEFAULT_AUDIENCE, 'nobody'),
      readSetting(KEY_CADENCE, 'daily'),
      readSetting(KEY_CADENCE_RANGE, 'weekly-to-daily'),
      readSetting(KEY_PROCESS_SHAPE, 'single-process'),
      global.AuroraEndpoints.ops.kryphocron.getSubstrateInfo().catch(() => null),
    ]);

    const codec = substrate && substrate.codecId ? substrate.codecId : null;
    const grid = container.querySelector('#kpolicy-grid');
    if (!grid) return;

    grid.innerHTML =
      // Active codec — read-only (§4-add.1). Laquna is the substrate's
      // default friction-encoding ContentCodec, not an internal module.
      '<div class="settings-card">' +
      '  <h3>' + esc(t('kryphocron.policy.codec_title')) + '</h3>' +
      '  <p class="settings-help">' + esc(t('kryphocron.policy.codec_help')) + '</p>' +
      '  <p class="stat-value">' + esc(codec || t('kryphocron.policy.codec_unknown')) + '</p>' +
      '</div>' +

      liveCard({
        key: KEY_ACCESS, selectId: 'kp-access',
        title: t('kryphocron.policy.access_title'),
        help: t('kryphocron.policy.access_help'),
        label: t('kryphocron.policy.access_label'),
        current: access.value, source: access.source,
        options: [
          { value: 'immediate', label: t('kryphocron.policy.access_immediate') },
          { value: 'delayed', label: t('kryphocron.policy.access_delayed') },
          { value: 'earned', label: t('kryphocron.policy.access_earned_pending'), disabled: true },
        ],
        extraHtml:
          '<div class="form-row"><label for="kp-access-delay">' +
          esc(t('kryphocron.policy.access_delay_label')) + '</label>' +
          '<input type="number" id="kp-access-delay" class="form-input" min="1" value="' +
          esc(delay.value) + '"></div>',
      }) +

      liveCard({
        key: KEY_DEFAULT_AUDIENCE, selectId: 'kp-defaud',
        title: t('kryphocron.policy.default_audience_title'),
        help: t('kryphocron.policy.default_audience_help'),
        label: t('kryphocron.policy.default_audience_label'),
        current: defAud.value, source: defAud.source,
        options: [
          { value: 'nobody', label: t('kryphocron.audiences.mode_nobody') },
          { value: 'list', label: t('kryphocron.audiences.mode_list') },
          { value: 'everyone', label: t('kryphocron.audiences.mode_everyone') },
          { value: 'followers', label: t('kryphocron.audiences.mode_followers') },
          { value: 'following', label: t('kryphocron.audiences.mode_following') },
        ],
      }) +

      liveCard({
        key: KEY_CADENCE, selectId: 'kp-cadence',
        title: t('kryphocron.policy.cadence_title'),
        help: t('kryphocron.policy.cadence_help'),
        label: t('kryphocron.policy.cadence_label'),
        current: cadence.value, source: cadence.source,
        options: [
          { value: 'hourly', label: t('kryphocron.laquna.cadence_hourly') },
          { value: 'daily', label: t('kryphocron.laquna.cadence_daily') },
          { value: 'weekly', label: t('kryphocron.laquna.cadence_weekly') },
          { value: 'manual-only', label: t('kryphocron.laquna.cadence_manual') },
        ],
        extraHtml:
          '<div class="form-row"><label for="kp-range">' +
          esc(t('kryphocron.policy.cadence_range_label')) + '</label>' +
          selectHtml('kp-range', [
            { value: 'weekly-to-daily', label: t('kryphocron.policy.range_weekly_daily') },
            { value: 'weekly-to-hourly', label: t('kryphocron.policy.range_weekly_hourly') },
            { value: 'no-override', label: t('kryphocron.policy.range_none') },
          ], range.value) +
          '<button class="btn btn-secondary" data-save="' + esc(KEY_CADENCE_RANGE) +
          '" data-select="kp-range">' + esc(t('kryphocron.policy.save_range')) + '</button></div>',
      }) +

      liveCard({
        key: KEY_PROCESS_SHAPE, selectId: 'kp-shape',
        title: t('kryphocron.policy.shape_title'),
        help: t('kryphocron.policy.shape_help'),
        label: t('kryphocron.policy.shape_label'),
        current: shape.value, source: shape.source,
        options: [
          { value: 'single-process', label: t('kryphocron.policy.shape_single') },
          { value: 'multi-process', label: t('kryphocron.policy.shape_multi') },
        ],
      }) +

      prereqCard(t('kryphocron.policy.ratelimits_title'), t('kryphocron.policy.ratelimits_pending')) +
      prereqCard(t('kryphocron.policy.overrides_title'), t('kryphocron.policy.overrides_pending'));
  }

  async function save(key, selectId, container) {
    const el = document.getElementById(selectId);
    if (!el) return;
    // One rationale covers the card's setting(s); the access-policy card also
    // persists its delay-days companion under the same audited save.
    const settings = [{ key: key, value: el.value }];
    if (key === KEY_ACCESS) {
      const delayEl = document.getElementById('kp-access-delay');
      if (delayEl && delayEl.value) {
        settings.push({ key: KEY_ACCESS_DELAY, value: parseInt(delayEl.value, 10) || 7 });
      }
    }
    const r = await global.AuroraAuditedSave.run({
      heading: t('kryphocron.policy.save_heading'),
      body: t('kryphocron.policy.save_body', { key: key, value: el.value }),
      settings: settings,
      successMessage: t('kryphocron.policy.save_success'),
    });
    // Refresh so the saved card's source-tier badge flips Default → Runtime (§8.2.1).
    if (r.saved) await load(container);
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('configKryphocronPolicy', { mount: mount });
  }
})(window);
