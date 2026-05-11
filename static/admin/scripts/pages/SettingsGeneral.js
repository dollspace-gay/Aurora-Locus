// Settings → General page (route: #settings/general).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.1.

(function (global) {
  'use strict';

  // Suffix appended to a setting value to indicate its source tier
  // (Runtime / File / Default / RecoveryMode). Runtime is the
  // operator-set normal case and renders bare; the other three are
  // informational annotations. Per V04_DESIGN.md §5.2.1 / §5.3.5.
  // Unknown values render bare so a future wire-additive source value
  // doesn't break rendering.
  //
  // Duplicated verbatim in pages/SettingsUiModes.js — the codebase
  // doesn't have a cross-page utility location for view helpers, and
  // manufacturing a module just for two callers is over-investment
  // per Step 2's scope.
  function settingSourceSuffix(source) {
    switch (source) {
      case 'Runtime':      return '';
      case 'Default':      return ' (default)';
      case 'File':         return ' (file)';
      case 'RecoveryMode': return ' (recovery override)';
      default:             return '';
    }
  }

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
      '      <div class="form-group"><label>Instance name <small class="settings-source-tag" id="sg-name-source"></small></label><input type="text" id="sg-name"' + (writable ? '' : ' disabled') + '></div>' +
      '      <div class="form-group"><label>Service URL <small class="settings-source-tag" id="sg-url-source"></small></label><input type="text" id="sg-url"' + (writable ? '' : ' disabled') + '></div>' +
      '      <div class="form-group"><label>Contact email <small class="settings-source-tag" id="sg-contact-source"></small></label><input type="email" id="sg-contact"' + (writable ? '' : ' disabled') + '></div>' +
      (writable ? '<button type="submit" class="btn-primary">Save changes</button>' : '<p class="settings-help">Read-only for non-SuperAdmin sessions.</p>') +
      '    </form>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Operational thresholds</h3>' +
      '    <form id="sg-thresholds">' +
      '      <div class="form-group"><label>Max blob size (MB) <small class="settings-source-tag" id="sg-blob-mb-source"></small></label><input type="number" id="sg-blob-mb" value="5"' + (writable ? '' : ' disabled') + '></div>' +
      '      <div class="form-group"><label>Account creation rate (per day) <small class="settings-source-tag" id="sg-acct-rate-source"></small></label><input type="number" id="sg-acct-rate" value="100"' + (writable ? '' : ' disabled') + '></div>' +
      (writable ? '<button type="submit" class="btn-primary">Save changes</button>' : '') +
      '    </form>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Registration</h3>' +
      '    <form id="sg-registration">' +
      '      <div class="form-group"><label class="checkbox-label"><input type="checkbox" id="sg-invite-required"' + (writable ? '' : ' disabled') + '> Require invite codes <small class="settings-source-tag" id="sg-invite-required-source"></small></label></div>' +
      '      <div class="form-group"><label class="checkbox-label"><input type="checkbox" id="sg-email-verification"' + (writable ? '' : ' disabled') + '> Require email verification <small class="settings-source-tag" id="sg-email-verification-source"></small></label></div>' +
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
      // Each setRuntimeSetting call lands its own audit entry. When the
      // form saves multiple keys, the toast's "View audit entry" link
      // points at the most-recent entry — pragmatic compromise that
      // beats omitting the link entirely or surfacing N toasts.
      let lastAuditEntryId = null;
      for (const [key, id] of fieldsByForm[formId] || []) {
        const el = document.getElementById(id);
        if (!el) continue;
        const value = el.type === 'checkbox' ? el.checked : el.value;
        const res = await ep.admin.setRuntimeSetting({ key: key, value: value, rationale: rationale });
        if (res && res.auditEntryId) lastAuditEntryId = res.auditEntryId;
      }
      global.AuroraToast.success('Settings saved.', lastAuditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(lastAuditEntryId),
        },
      } : undefined);
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('settingsGeneral', { mount: mount });
})(window);
