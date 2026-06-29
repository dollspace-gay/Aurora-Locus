// Recovery mode status page (route: #configuration/recovery-mode).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §7.3.2 (Arc H), translated to the
// substrate idiom: recovery mode is entered via the AURORA_RECOVERY_MODE
// env var at process start and exited by restarting without it — a
// deliberate corruption-recovery fail-safe, NOT a runtime toggle. So this
// is a read-only STATUS + documented-procedure surface (no fake controls,
// per §7.6). It detects the active state from the signal the substrate
// already exposes: getRuntimeSetting('moderation-mode').source ===
// 'RecoveryMode'. The §16 design note records this entry/exit-UX →
// status-surface translation; future Arc H work doesn't re-litigate it.

(function (global) {
  'use strict';

  function T(key, params) { return global.t ? global.t(key, params) : key; }
  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">' + esc(T('recoveryMode.crumb')) +
      '</a> <span class="breadcrumb-sep">›</span> ' + esc(T('recoveryMode.title')) + '</nav>' +
      '<header class="page-header"><div><h2>' + esc(T('recoveryMode.title')) +
      ' <span class="role-tag">SuperAdmin only</span></h2>' +
      '<p class="page-subtitle">' + esc(T('recoveryMode.subtitle')) + '</p></div></header>' +
      '<div id="recovery-status" class="settings-card">' + global.AuroraSkeleton.lines(2) + '</div>' +
      '<div class="settings-card">' +
      '  <h3>' + esc(T('recoveryMode.about_heading')) + '</h3>' +
      '  <p class="settings-help">' + esc(T('recoveryMode.about_body')) + '</p>' +
      '  <ul class="settings-help">' +
      '    <li>' + esc(T('recoveryMode.effect_bypass')) + '</li>' +
      '    <li>' + esc(T('recoveryMode.effect_audited')) + '</li>' +
      '    <li>' + esc(T('recoveryMode.effect_preserved')) + '</li>' +
      '  </ul>' +
      '</div>' +
      '<div class="settings-card">' +
      '  <h3>' + esc(T('recoveryMode.procedure_heading')) + '</h3>' +
      '  <p class="settings-help">' + esc(T('recoveryMode.procedure_intro')) + '</p>' +
      '  <p class="settings-help"><strong>' + esc(T('recoveryMode.enter_label')) + '</strong> ' +
      esc(T('recoveryMode.enter_body')) + ' <code>AURORA_RECOVERY_MODE=true</code></p>' +
      '  <p class="settings-help"><strong>' + esc(T('recoveryMode.exit_label')) + '</strong> ' +
      esc(T('recoveryMode.exit_body')) + '</p>' +
      '  <p class="settings-help">' + esc(T('recoveryMode.no_runtime_note')) + '</p>' +
      '</div>';

    await loadStatus();
    return {};
  }

  async function loadStatus() {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('recovery-status');
    if (!c || !ep) return;
    let data;
    try {
      // The substrate surfaces recovery mode as a source-tier override on
      // the moderation-mode read (SettingSource::RecoveryMode). No dedicated
      // status XRPC is needed — this reads what the substrate already shows.
      data = await ep.admin.getRuntimeSetting('moderation-mode');
    } catch (e) {
      global.AuroraInlineError.mount(c, {
        message: T('recoveryMode.error') + (e && e.message ? ': ' + e.message : ''),
        onRetry: loadStatus,
      });
      return;
    }
    const active = !!(data && data.source === 'RecoveryMode');
    // 'takedown' = the red variant — recovery mode bypasses write authz, so
    // an active state warrants the strongest alert colour. Inactive = the
    // green 'active' (normal operation).
    const badge = active
      ? global.AuroraStatusBadge.render('takedown', T('recoveryMode.status_active'))
      : global.AuroraStatusBadge.render('active', T('recoveryMode.status_inactive'));
    c.innerHTML = '<h3>' + esc(T('recoveryMode.status_heading')) + '</h3>' +
      '<p>' + badge + '</p>' +
      '<p class="settings-help">' +
      esc(active ? T('recoveryMode.status_active_help') : T('recoveryMode.status_inactive_help')) +
      '</p>';
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configRecoveryMode', { mount: mount });
})(window);
