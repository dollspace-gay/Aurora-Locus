// Configuration → General page (route: #configuration/general).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.1.

(function (global) {
  'use strict';

  // Source-tier suffix (Runtime / File / Default / RecoveryMode) per
  // V04_DESIGN.md §5.2.1 / §5.3.5. §8.2.1 consolidated the formerly-
  // duplicated helper into the AuroraSourceTier primitive; this thin
  // delegate keeps the existing call sites unchanged.
  function settingSourceSuffix(source) {
    return global.AuroraSourceTier.suffix(source);
  }

  // #299 — these `general.*` keys are NOT in the substrate runtime-settings
  // registry and nothing in the runtime consumes them, so a save would either
  // be rejected ("unknown runtime setting key") or accepted-but-no-op. Until
  // the backend actually consumes them (deferred feature work, tracked
  // separately), the page is a READ-ONLY display of current values rather than
  // presenting a save affordance that can't work. Inputs are disabled and the
  // save buttons are replaced with a deferral note (Arc-G-stub pattern).
  async function mount({ container }) {
    const note =
      '<p class="settings-help">These server settings are shown read-only. ' +
      'Editing them from the admin UI is not yet wired to the runtime and ' +
      'arrives in a future release.</p>';
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> General</nav>' +
      '<header class="page-header"><div><h2>General settings</h2><p class="page-subtitle">Server identity and basic configuration</p></div></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Server identity</h3>' +
      '    <div class="form-group"><label>Instance name <small class="settings-source-tag" id="sg-name-source"></small></label><input type="text" id="sg-name" disabled></div>' +
      '    <div class="form-group"><label>Service URL <small class="settings-source-tag" id="sg-url-source"></small></label><input type="text" id="sg-url" disabled></div>' +
      '    <div class="form-group"><label>Contact email <small class="settings-source-tag" id="sg-contact-source"></small></label><input type="email" id="sg-contact" disabled></div>' +
      note +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Operational thresholds</h3>' +
      '    <div class="form-group"><label>Max blob size (MB) <small class="settings-source-tag" id="sg-blob-mb-source"></small></label><input type="number" id="sg-blob-mb" value="5" disabled></div>' +
      '    <div class="form-group"><label>Account creation rate (per day) <small class="settings-source-tag" id="sg-acct-rate-source"></small></label><input type="number" id="sg-acct-rate" value="100" disabled></div>' +
      note +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Registration</h3>' +
      '    <div class="form-group"><label class="checkbox-label"><input type="checkbox" id="sg-invite-required" disabled> Require invite codes <small class="settings-source-tag" id="sg-invite-required-source"></small></label></div>' +
      '    <div class="form-group"><label class="checkbox-label"><input type="checkbox" id="sg-email-verification" disabled> Require email verification <small class="settings-source-tag" id="sg-email-verification-source"></small></label></div>' +
      note +
      '  </div>' +
      '</div>';

    await loadValues();
    return {};
  }

  async function loadValues() {
    // Read existing settings from runtime settings keyed namespace.
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    const fields = [
      ['general.instance-name', 'sg-name', 'Aurora Locus PDS'],
      ['general.service-url', 'sg-url', ''],
      ['general.contact-email', 'sg-contact', ''],
      ['general.max-blob-mb', 'sg-blob-mb', 5],
      ['general.account-rate-per-day', 'sg-acct-rate', 100],
    ];
    for (const [key, id, defaultV] of fields) {
      try {
        const data = await ep.admin.getRuntimeSetting(key);
        const v = data && data.value;
        const el = document.getElementById(id);
        if (el) el.value = (v != null ? v : defaultV);
        const srcEl = document.getElementById(id + '-source');
        if (srcEl) srcEl.textContent = settingSourceSuffix(data && data.source);
      } catch (e) {
        const el = document.getElementById(id);
        if (el) el.value = defaultV;
      }
    }
    // Booleans
    for (const [key, id] of [['general.invite-required', 'sg-invite-required'], ['general.email-verification', 'sg-email-verification']]) {
      try {
        const data = await ep.admin.getRuntimeSetting(key);
        const el = document.getElementById(id);
        if (el) el.checked = !!(data && (data.value === true || data.value === 'true'));
        const srcEl = document.getElementById(id + '-source');
        if (srcEl) srcEl.textContent = settingSourceSuffix(data && data.source);
      } catch (e) { /* leave unchecked */ }
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configGeneral', { mount: mount });
})(window);
