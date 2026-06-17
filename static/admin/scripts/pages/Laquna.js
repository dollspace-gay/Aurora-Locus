// Laquna page (§6.4.2) — the one-button rotation surface. Route
// #kryphocron/laquna, Admin+ (rotation is operator-tier; cadence config is
// SuperAdmin-write). "Laquna" (capital L) is the project name for the
// substrate's default friction-encoding codec; the page surfaces rotation
// of that codec's generation, not encryption.
//
// Cards: current state (getRotationStatus) + the manual-rotation button
// (triggerRotation, with a typed-confirm + rationale modal) that streams
// getRotationProgress every 5s until terminal, with a cancel affordance
// (cancelRotation); and the cadence-policy cards (rotation-cadence +
// account-cadence-range, SuperAdmin-write). Reads/controls the #225 cohort.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);
  const K = () => global.AuroraEndpoints.ops.kryphocron;

  const KEY_CADENCE = 'kryphocron.laquna.rotation-cadence';
  const KEY_RANGE = 'kryphocron.laquna.account-cadence-range';

  let statusPoll = null;
  let progressPoll = null;

  function clearPolls() {
    if (statusPoll) clearInterval(statusPoll);
    if (progressPoll) clearInterval(progressPoll);
    statusPoll = null;
    progressPoll = null;
  }

  async function readSetting(key, dflt) {
    try {
      const out = await global.AuroraEndpoints.admin.getRuntimeSetting(key);
      const v = out && (out.value !== undefined ? out.value : out);
      return v == null || v === '' ? dflt : String(v);
    } catch (e) { return dflt; }
  }

  // §10.4.3 — canonical timestamp rendering (returns a <time> element).
  function ts(value, context) {
    return global.AuroraTimestamp.render({ value: value, context: context });
  }

  async function mount({ container }) {
    const isSuper = global.AuroraSession && global.AuroraSession.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#kryphocron/overview">' + esc(t('kryphocron.overview.title')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('kryphocron.laquna.title')) +
      '</nav>' +
      '<header class="page-header"><h2>' + esc(t('kryphocron.laquna.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('kryphocron.laquna.subtitle')) + '</p></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card" id="lq-state">' + global.AuroraSkeleton.lines(4) + '</div>' +
      '  <div class="settings-card" id="lq-action">' + global.AuroraSkeleton.lines(3) + '</div>' +
      '  <div class="settings-card" id="lq-cadence">' + global.AuroraSkeleton.lines(3) + '</div>' +
      '</div>';

    await renderState();
    await renderCadence(isSuper);
    statusPoll = setInterval(renderState, 30000);
    return { unmount: clearPolls };
  }

  async function renderState() {
    const stateCard = document.getElementById('lq-state');
    const actionCard = document.getElementById('lq-action');
    if (!stateCard) return;
    let r;
    try { r = await K().getRotationStatus(); }
    catch (e) {
      stateCard.innerHTML = '<h3>' + esc(t('kryphocron.laquna.state_title')) + '</h3>' +
        '<div id="lq-state-err"></div>';
      global.AuroraInlineError.mount(stateCard.querySelector('#lq-state-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: renderState,
      });
      return;
    }
    stateCard.innerHTML =
      '<h3>' + esc(t('kryphocron.laquna.state_title')) + '</h3>' +
      '<dl class="kv-list">' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.generation')) +
        '</dt><dd><code>' + esc(r.generationMark || '—') + '</code></dd></div>' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.last_slug')) +
        '</dt><dd>' + ts(r.lastSlugRotation, 'detail') + '</dd></div>' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.last_rewrite')) +
        '</dt><dd>' + (r.lastRecordRewriteCompleted ? ts(r.lastRecordRewriteCompleted, 'detail')
          : esc(t('kryphocron.laquna.never'))) + '</dd></div>' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.cadence_current')) +
        '</dt><dd>' + esc(r.cadence || '—') + '</dd></div>' +
      '</dl>';

    // The action card reflects whether a rewrite is in flight.
    if (actionCard) {
      if (r.rewriteInProgress) {
        startProgress();
      } else {
        renderRotateButton();
      }
    }
  }

  function renderRotateButton() {
    const card = document.getElementById('lq-action');
    if (!card) return;
    if (progressPoll) { clearInterval(progressPoll); progressPoll = null; }
    card.innerHTML =
      '<h3>' + esc(t('kryphocron.laquna.rotate_title')) + '</h3>' +
      '<p class="settings-help">' + esc(t('kryphocron.laquna.rotate_explainer')) + '</p>' +
      '<p class="settings-help">' + esc(t('kryphocron.laquna.rotate_not')) + '</p>' +
      '<div class="form-actions">' +
      '  <button class="btn btn-primary" id="lq-rotate">' +
        esc(t('kryphocron.laquna.rotate_button')) + '</button>' +
      '</div>';
    const btn = document.getElementById('lq-rotate');
    if (btn) btn.addEventListener('click', doRotate);
  }

  async function doRotate() {
    const res = await global.AuroraModal.destructiveConfirm({
      heading: t('kryphocron.laquna.rotate_confirm_heading'),
      body: t('kryphocron.laquna.rotate_confirm_body'),
      rationaleRequired: true,
      typedConfirmGate: 'ROTATE',
      confirmLabel: t('kryphocron.laquna.rotate_button'),
    });
    if (!res.confirmed) return;
    try {
      await K().triggerRotation();
      global.AuroraToast.success(t('kryphocron.laquna.rotate_started'));
      startProgress();
    } catch (e) {
      if (e && (e.status === 409 || e.code === 'RotationInProgress')) {
        global.AuroraToast.danger(t('kryphocron.laquna.rotate_in_progress'));
        startProgress();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  function startProgress() {
    renderProgress();
    if (progressPoll) clearInterval(progressPoll);
    progressPoll = setInterval(renderProgress, 5000); // §6.4.2: 5s during rotation
  }

  async function renderProgress() {
    const card = document.getElementById('lq-action');
    if (!card) return;
    let p;
    try { p = await K().getRotationProgress(); }
    catch (e) { return; }
    if (!p || !p.running) {
      // Terminal — back to the button + refresh the state card.
      renderRotateButton();
      renderState();
      return;
    }
    card.innerHTML =
      '<h3>' + esc(t('kryphocron.laquna.rotating_title')) + '</h3>' +
      '<dl class="kv-list">' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.records_processed')) +
        '</dt><dd>' + esc(String(p.recordsProcessed != null ? p.recordsProcessed : 0)) + '</dd></div>' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.records_rewritten')) +
        '</dt><dd>' + esc(String(p.recordsRewritten != null ? p.recordsRewritten : 0)) + '</dd></div>' +
      '<div class="kv-row"><dt>' + esc(t('kryphocron.laquna.started_at')) +
        '</dt><dd>' + ts(p.startedAt, 'detail') + '</dd></div>' +
      '</dl>' +
      (p.cancelRequested
        ? '<p class="settings-help">' + esc(t('kryphocron.laquna.cancel_pending')) + '</p>'
        : '<div class="form-actions"><button class="btn btn-secondary" id="lq-cancel">' +
          esc(t('kryphocron.laquna.cancel_button')) + '</button></div>');
    const btn = document.getElementById('lq-cancel');
    if (btn) btn.addEventListener('click', doCancel);
  }

  async function doCancel() {
    const res = await global.AuroraModal.form({
      heading: t('kryphocron.laquna.cancel_confirm_heading'),
      body: t('kryphocron.laquna.cancel_confirm_body'),
      submitLabel: t('kryphocron.laquna.cancel_button'),
      fields: [],
    });
    if (!res.submitted) return;
    try {
      await K().cancelRotation();
      global.AuroraToast.success(t('kryphocron.laquna.cancel_requested'));
      renderProgress();
    } catch (e) {
      if (e && (e.status === 409 || e.code === 'NoRotationInProgress')) {
        global.AuroraToast.danger(t('kryphocron.laquna.cancel_none'));
        renderRotateButton();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  async function renderCadence(isSuper) {
    const card = document.getElementById('lq-cadence');
    if (!card) return;
    const [cadence, range] = await Promise.all([
      readSetting(KEY_CADENCE, 'daily'),
      readSetting(KEY_RANGE, 'weekly-to-daily'),
    ]);
    const cadenceOpts = [
      ['hourly', t('kryphocron.laquna.cadence_hourly')],
      ['daily', t('kryphocron.laquna.cadence_daily')],
      ['weekly', t('kryphocron.laquna.cadence_weekly')],
      ['manual-only', t('kryphocron.laquna.cadence_manual')],
    ];
    const rangeOpts = [
      ['weekly-to-daily', t('kryphocron.policy.range_weekly_daily')],
      ['weekly-to-hourly', t('kryphocron.policy.range_weekly_hourly')],
      ['no-override', t('kryphocron.policy.range_none')],
    ];
    function sel(id, opts, cur) {
      return '<select id="' + id + '" class="form-select"' + (isSuper ? '' : ' disabled') + '>' +
        opts.map((o) => '<option value="' + esc(o[0]) + '"' + (o[0] === cur ? ' selected' : '') +
          '>' + esc(o[1]) + '</option>').join('') + '</select>';
    }
    card.innerHTML =
      '<h3>' + esc(t('kryphocron.laquna.cadence_title')) + '</h3>' +
      '<p class="settings-help">' + esc(t('kryphocron.laquna.cadence_help')) + '</p>' +
      '<div class="form-row"><label>' + esc(t('kryphocron.laquna.cadence_label')) + '</label>' +
        sel('lq-cadence-sel', cadenceOpts, cadence) + '</div>' +
      '<div class="form-row"><label>' + esc(t('kryphocron.policy.cadence_range_label')) + '</label>' +
        sel('lq-range-sel', rangeOpts, range) + '</div>' +
      (isSuper
        ? '<div class="form-actions"><button class="btn btn-primary" id="lq-cadence-save">' +
          esc(t('common.save')) + '</button></div>'
        : '<p class="settings-help">' + esc(t('kryphocron.laquna.cadence_superadmin')) + '</p>') +
      '<p class="settings-help"><a href="#kryphocron/laquna/history">' +
        esc(t('kryphocron.laquna.history_link')) + '</a></p>';
    const saveBtn = document.getElementById('lq-cadence-save');
    if (saveBtn) saveBtn.addEventListener('click', saveCadence);
  }

  async function saveCadence() {
    const cadence = document.getElementById('lq-cadence-sel');
    const range = document.getElementById('lq-range-sel');
    await global.AuroraAuditedSave.run({
      heading: t('kryphocron.laquna.cadence_save_heading'),
      body: t('kryphocron.laquna.cadence_save_body'),
      settings: [
        { key: KEY_CADENCE, value: cadence.value },
        { key: KEY_RANGE, value: range.value },
      ],
      successMessage: t('kryphocron.laquna.cadence_saved'),
    });
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('kryphocronLaquna', { mount: mount });
  }
})(window);
