// Settings → General page (route: #settings/general).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.1.

(function (global) {
  'use strict';

  async function mount({ container }) {
    const session = global.AuroraSession;
    const writable = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#settings/general">Settings</a> <span class="breadcrumb-sep">›</span> General</nav>' +
      '<header class="page-header"><div><h2>General settings</h2><p class="page-subtitle">Server identity and basic configuration</p></div></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Server identity</h3>' +
      '    <form id="sg-identity">' +
      '      <div class="form-group"><label>Instance name</label><input type="text" id="sg-name"' + (writable ? '' : ' disabled') + '></div>' +
      '      <div class="form-group"><label>Service URL</label><input type="text" id="sg-url"' + (writable ? '' : ' disabled') + '></div>' +
      '      <div class="form-group"><label>Contact email</label><input type="email" id="sg-contact"' + (writable ? '' : ' disabled') + '></div>' +
      (writable ? '<button type="submit" class="btn-primary">Save changes</button>' : '<p class="settings-help">Read-only for non-SuperAdmin sessions.</p>') +
      '    </form>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Operational thresholds</h3>' +
      '    <form id="sg-thresholds">' +
      '      <div class="form-group"><label>Max blob size (MB)</label><input type="number" id="sg-blob-mb" value="5"' + (writable ? '' : ' disabled') + '></div>' +
      '      <div class="form-group"><label>Account creation rate (per day)</label><input type="number" id="sg-acct-rate" value="100"' + (writable ? '' : ' disabled') + '></div>' +
      (writable ? '<button type="submit" class="btn-primary">Save changes</button>' : '') +
      '    </form>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Registration</h3>' +
      '    <form id="sg-registration">' +
      '      <div class="form-group"><label class="checkbox-label"><input type="checkbox" id="sg-invite-required"' + (writable ? '' : ' disabled') + '> Require invite codes</label></div>' +
      '      <div class="form-group"><label class="checkbox-label"><input type="checkbox" id="sg-email-verification"' + (writable ? '' : ' disabled') + '> Require email verification</label></div>' +
      (writable ? '<button type="submit" class="btn-primary">Save changes</button>' : '') +
      '    </form>' +
      '  </div>' +
      '</div>';

    await loadValues();
    if (writable) {
      ['sg-identity', 'sg-thresholds', 'sg-registration'].forEach((id) => {
        const f = document.getElementById(id);
        if (f) f.addEventListener('submit', (e) => { e.preventDefault(); saveCard(id); });
      });
    }
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
      } catch (e) { /* leave unchecked */ }
    }
  }

  async function saveCard(formId) {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    const fieldsByForm = {
      'sg-identity': [
        ['general.instance-name', 'sg-name'],
        ['general.service-url', 'sg-url'],
        ['general.contact-email', 'sg-contact'],
      ],
      'sg-thresholds': [
        ['general.max-blob-mb', 'sg-blob-mb'],
        ['general.account-rate-per-day', 'sg-acct-rate'],
      ],
      'sg-registration': [
        ['general.invite-required', 'sg-invite-required'],
        ['general.email-verification', 'sg-email-verification'],
      ],
    };
    const rationale = 'Routine config update via Settings → General';
    try {
      for (const [key, id] of fieldsByForm[formId] || []) {
        const el = document.getElementById(id);
        if (!el) continue;
        const value = el.type === 'checkbox' ? el.checked : el.value;
        await ep.admin.setRuntimeSetting({ key: key, value: value, rationale: rationale });
      }
      global.AuroraToast.success('Settings saved.');
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('settingsGeneral', { mount: mount });
})(window);
