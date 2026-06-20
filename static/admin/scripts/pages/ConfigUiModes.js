// Configuration → UI & modes page (route: #configuration/ui-modes).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.2.

(function (global) {
  'use strict';

  // Source-tier suffix per V04_DESIGN.md §5.2.1 / §5.3.5. §8.2.1
  // consolidated the formerly-duplicated helper into the AuroraSourceTier
  // primitive; this thin delegate keeps the existing call sites unchanged.
  function settingSourceSuffix(source) {
    return global.AuroraSourceTier.suffix(source);
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
      '    <p class="settings-help">Choose an installed theme, or follow the deployment default.</p>' +
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
      '</div>' +
      // Branding — SuperAdmin-only login-splash customization (deployment-wide).
      // Below Moderation mode, above Installed Themes. Hidden for moderator/admin
      // (matches the superadmin-only set-deployment-default gating).
      (isSuper ?
        '<hr class="config-section-divider">' +
        '<section class="branding-section">' +
        '  <h3>Branding</h3>' +
        '  <p class="settings-help">Customize the login splash imagery (deployment-wide). Host an asset under <code>static/branding/</code> and reference <code>/static/branding/&lt;file&gt;</code>, or use any external URL.</p>' +
        '  <label class="branding-field">Login logo URL' +
        '    <input type="text" id="branding-logo" placeholder="/static/branding/logo.png">' +
        '  </label>' +
        '  <label class="branding-field">Login banner image URL' +
        '    <input type="text" id="branding-banner" placeholder="https://your-cdn.example/banner.png">' +
        '  </label>' +
        '  <button type="button" class="btn-primary" id="branding-save">Save branding</button>' +
        '</section>'
        : '') +
      // Installed Themes — folded in from the former standalone Themes page
      // (#322). The row list is owned by AuroraInstalledThemes; set-default is
      // superadmin-only, the rest is read-only for moderator/admin.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Installed Themes</h3>' +
      '  <div id="installed-themes"></div>' +
      '</section>';

    if (global.AuroraThemeToggle) global.AuroraThemeToggle.mountDropdown(document.getElementById('ui-theme-toggle'));
    if (global.AuroraInstalledThemes) global.AuroraInstalledThemes.mount(document.getElementById('installed-themes'), isSuper);

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
      await loadBranding();
    }
    return {};
  }

  // Load the current login-branding URLs into the section's inputs and wire the
  // save button (SuperAdmin-only section).
  async function loadBranding() {
    const ep = global.AuroraEndpoints;
    const logoEl = document.getElementById('branding-logo');
    const bannerEl = document.getElementById('branding-banner');
    if (!ep || !logoEl || !bannerEl) return;
    try {
      const d = await ep.admin.getRuntimeSetting('branding.login-logo-url');
      if (d && typeof d.value === 'string') logoEl.value = d.value;
    } catch (e) { /* leave blank */ }
    try {
      const d = await ep.admin.getRuntimeSetting('branding.login-banner-image-url');
      if (d && typeof d.value === 'string') bannerEl.value = d.value;
    } catch (e) { /* leave blank */ }
    const btn = document.getElementById('branding-save');
    if (btn) btn.addEventListener('click', saveBranding);
  }

  // Save both branding URLs deployment-wide. Cosmetic (light confirm, no typed
  // rationale) like the theme deployment-default save (#308), but still lands an
  // audit-chain entry per setting via the auto-filled rationale.
  async function saveBranding() {
    const logo = document.getElementById('branding-logo').value.trim();
    const banner = document.getElementById('branding-banner').value.trim();
    await global.AuroraAuditedSave.run({
      heading: 'Save login branding',
      body: 'Update the login splash logo and banner? This is deployment-wide — every operator sees it on the login page.',
      confirmLabel: 'Save branding',
      cosmetic: true,
      autoRationale: 'cosmetic setting: login branding URLs updated',
      settings: [
        { key: 'branding.login-logo-url', value: logo },
        { key: 'branding.login-banner-image-url', value: banner },
      ],
      successMessage: 'Login branding saved.',
    });
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
