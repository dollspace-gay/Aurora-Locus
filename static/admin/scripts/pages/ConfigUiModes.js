// Configuration → UI & modes page (route: #configuration/ui-modes).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.2.

(function (global) {
  'use strict';

  // Suffix appended to a setting value to indicate its source tier
  // (Runtime / File / Default / RecoveryMode). Runtime is the
  // operator-set normal case and renders bare; the other three are
  // informational annotations. Per V04_DESIGN.md §5.2.1 / §5.3.5.
  // Unknown values render bare so a future wire-additive source value
  // doesn't break rendering.
  //
  // Duplicated verbatim in pages/SettingsGeneral.js — the codebase
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
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/ui-modes">Configuration</a> <span class="breadcrumb-sep">›</span> UI & modes</nav>' +
      '<header class="page-header"><div><h2>UI & modes</h2><p class="page-subtitle">Theme, language, deployment moderation mode</p></div></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Theme</h3>' +
      '    <div id="ui-theme-toggle"></div>' +
      '    <p class="settings-help">System uses your operating system preference.</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Language</h3>' +
      '    <select id="ui-language"><option value="en">English</option></select>' +
      '    <p class="settings-help">Other languages may be available as locale files are added.</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Moderation mode <span class="role-tag">SuperAdmin only</span></h3>' +
      '    <fieldset>' +
      '      <legend>Mode</legend>' +
      '      <label><input type="radio" name="mod-mode" value="full"' + (isSuper ? '' : ' disabled') + '> Full</label>' +
      '      <label style="margin-left: 0.75rem;"><input type="radio" name="mod-mode" value="reduced"' + (isSuper ? '' : ' disabled') + '> Reduced</label>' +
      '      <label style="margin-left: 0.75rem;"><input type="radio" name="mod-mode" value="disabled"' + (isSuper ? '' : ' disabled') + '> Disabled</label>' +
      '      <p>Current: <strong id="mod-mode-current">Loading…</strong></p>' +
      '      <label style="display: block; margin-top: 0.5rem;">Redirect URL (when disabled)' +
      '        <input type="text" id="mod-mode-redirect" style="width:100%;"' + (isSuper ? '' : ' disabled') + '></label>' +
      '      <label style="display: block; margin-top: 0.5rem;">Rationale (required)' +
      '        <textarea id="mod-mode-rationale" rows="2" style="width:100%;"' + (isSuper ? '' : ' disabled') + '></textarea></label>' +
      (isSuper ? '<button type="button" class="btn-primary" id="mod-mode-save">Save mode change</button>' : '<p class="settings-help">SuperAdmin role required to change deployment-wide moderation mode.</p>') +
      '    </fieldset>' +
      '  </div>' +
      '</div>';

    if (global.AuroraThemeToggle) global.AuroraThemeToggle.mountFull(document.getElementById('ui-theme-toggle'));

    const langSel = document.getElementById('ui-language');
    if (langSel) {
      try {
        const reg = await fetch('/admin/i18n/locales.json').then((r) => r.ok ? r.json() : null);
        if (reg && Array.isArray(reg.available) && reg.available.length) {
          langSel.innerHTML = reg.available.map((l) =>
            '<option value="' + l.code + '">' + (global.AuroraDom ? global.AuroraDom.esc(l.name) : l.name) + '</option>').join('');
        }
      } catch (e) { /* leave English-only */ }
      langSel.value = global.AuroraSettings ? global.AuroraSettings.language() : 'en';
      langSel.addEventListener('change', () => {
        if (global.AuroraSettings) global.AuroraSettings.setLanguage(langSel.value);
      });
    }

    await loadModerationMode();
    if (isSuper) {
      const saveBtn = document.getElementById('mod-mode-save');
      if (saveBtn) saveBtn.addEventListener('click', saveModerationMode);
    }
    return {};
  }

  async function loadModerationMode() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.admin.getRuntimeSetting('moderation-mode');
      const value = (data && typeof data.value === 'string') ? data.value : 'full';
      const cur = document.getElementById('mod-mode-current');
      if (cur) cur.textContent = value + settingSourceSuffix(data && data.source);
      const radio = document.querySelector('input[name="mod-mode"][value="' + value + '"]');
      if (radio) radio.checked = true;
      if (global.AuroraSettings) global.AuroraSettings.setModerationModeCache(value);
    } catch (e) { /* ignore */ }
    try {
      const data = await ep.admin.getRuntimeSetting('moderation-mode-redirect-url');
      const v = (data && typeof data.value === 'string') ? data.value : '';
      const input = document.getElementById('mod-mode-redirect');
      if (input) input.value = v;
    } catch (e) { /* ignore */ }
  }

  async function saveModerationMode() {
    const selected = document.querySelector('input[name="mod-mode"]:checked');
    if (!selected) { global.AuroraToast.warning('Select a mode.'); return; }
    const rationale = document.getElementById('mod-mode-rationale').value.trim();
    if (!rationale) { global.AuroraToast.warning('Rationale is required.'); return; }
    const confirmResult = await global.AuroraModal.destructiveConfirm({
      heading: 'Switch moderation mode',
      body: 'Switch moderation mode to "' + selected.value + '"? This affects all operators using this PDS.',
      confirmLabel: 'Switch mode',
    });
    if (!confirmResult.confirmed) return;
    const redirect = document.getElementById('mod-mode-redirect').value.trim();
    try {
      await global.AuroraEndpoints.admin.setRuntimeSetting({ key: 'moderation-mode', value: selected.value, rationale: rationale });
      // Two setRuntimeSetting calls land two audit entries. Link the
      // toast to the most-recent (redirect-url) entry per the same
      // pragmatic last-entry rule used in SettingsGeneral.saveCard.
      const res = await global.AuroraEndpoints.admin.setRuntimeSetting({ key: 'moderation-mode-redirect-url', value: redirect, rationale: rationale });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Mode change saved. Sidebar may re-render.', auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      await loadModerationMode();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configUiModes', { mount: mount });
})(window);
