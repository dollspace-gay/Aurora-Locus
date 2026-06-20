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

  // Branding assets (#329): the runtime key, per-type size cap, and accepted
  // upload types for each login-splash asset.
  const BRANDING = {
    logo: { key: 'branding.login-logo-url', max: 1048576, label: '1MB' },
    banner: { key: 'branding.login-banner-image-url', max: 5242880, label: '5MB' },
  };
  const BRANDING_ACCEPT = 'image/png,image/jpeg,image/svg+xml,image/webp';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s);
  }

  // One compound asset group: file upload + URL field + clear, with a preview
  // thumbnail and a status line. The URL field keeps the id `branding-<type>`
  // so the "Save URLs" flow reads it.
  function brandingAsset(type, label, placeholder) {
    return '<div class="branding-asset" data-asset="' + type + '">' +
      '  <div class="branding-asset-label">' + esc(label) + '</div>' +
      '  <div class="branding-controls">' +
      '    <input type="file" class="branding-file" data-asset="' + type + '" accept="' + BRANDING_ACCEPT + '">' +
      '    <input type="text" class="branding-url" id="branding-' + type + '" placeholder="' + esc(placeholder) + '">' +
      '    <button type="button" class="btn-secondary btn-sm branding-clear" data-asset="' + type + '">Clear</button>' +
      '  </div>' +
      '  <div class="branding-row2">' +
      '    <img class="branding-thumb" data-asset="' + type + '" alt="" hidden>' +
      '    <span class="branding-status" data-asset="' + type + '"></span>' +
      '  </div>' +
      '</div>';
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
        '  <p class="settings-help">Customize the login splash imagery (deployment-wide). Upload an image, or paste a URL (host it under <code>static/branding/</code> and reference <code>/static/branding/&lt;file&gt;</code>, or any external URL).</p>' +
        brandingAsset('logo', 'Login logo', '/static/branding/logo.png') +
        brandingAsset('banner', 'Login banner image', 'https://your-cdn.example/banner.png') +
        '  <button type="button" class="btn-primary" id="branding-save">Save URLs</button>' +
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

  // Load the current branding URLs into each asset group's URL field +
  // preview, and wire the file uploads, clear buttons, and save (SuperAdmin
  // section).
  async function loadBranding() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    for (const type of ['logo', 'banner']) {
      try {
        const d = await ep.admin.getRuntimeSetting(BRANDING[type].key);
        const val = (d && typeof d.value === 'string') ? d.value : '';
        const urlEl = document.getElementById('branding-' + type);
        if (urlEl) urlEl.value = val;
        setPreview(type, val);
      } catch (e) { /* leave blank */ }
    }
    document.querySelectorAll('.branding-file').forEach((el) => {
      el.addEventListener('change', () => onUpload(el.dataset.asset, el.files && el.files[0]));
    });
    document.querySelectorAll('.branding-clear').forEach((el) => {
      el.addEventListener('click', () => onClear(el.dataset.asset));
    });
    const btn = document.getElementById('branding-save');
    if (btn) btn.addEventListener('click', saveBranding);
  }

  function setPreview(type, url) {
    const img = document.querySelector('.branding-thumb[data-asset="' + type + '"]');
    if (!img) return;
    if (url) { img.src = url; img.hidden = false; }
    else { img.removeAttribute('src'); img.hidden = true; }
  }

  function setStatus(type, msg, isError) {
    const el = document.querySelector('.branding-status[data-asset="' + type + '"]');
    if (!el) return;
    el.textContent = msg || '';
    el.classList.toggle('is-error', !!isError);
  }

  // Upload a chosen file directly to the substrate (raw body + assetType query,
  // matching the XRPC's uploadBlob-style contract). The substrate writes it,
  // repoints the runtime setting, and audits — so on success we just reflect
  // the served URL into the field + preview (no separate save).
  async function onUpload(type, file) {
    if (!file || !BRANDING[type]) return;
    const spec = BRANDING[type];
    if (BRANDING_ACCEPT.split(',').indexOf(file.type) === -1) {
      setStatus(type, 'Unsupported type — use PNG, JPEG, SVG, or WebP.', true);
      return;
    }
    if (file.size > spec.max) {
      setStatus(type, 'Too large — max ' + spec.label + '.', true);
      return;
    }
    setStatus(type, 'Uploading…', false);
    try {
      const token = global.AuroraSession
        ? global.AuroraSession.token()
        : localStorage.getItem('aurora-admin-token');
      const qs = '?assetType=' + encodeURIComponent(type) +
        '&rationale=' + encodeURIComponent('branding upload: ' + type);
      const res = await fetch('/xrpc/tools.aurora.superadmin.uploadBrandingAsset' + qs, {
        method: 'POST',
        headers: { 'Content-Type': file.type, Authorization: 'Bearer ' + (token || '') },
        body: file,
      });
      if (!res.ok) {
        let msg = 'Upload failed (' + res.status + ')';
        try { const j = await res.json(); if (j && j.message) msg = j.message; } catch (e) { /* non-JSON */ }
        setStatus(type, msg, true);
        return;
      }
      const data = await res.json();
      const urlEl = document.getElementById('branding-' + type);
      if (urlEl) urlEl.value = data.url || '';
      setPreview(type, data.url || '');
      setStatus(type, 'Uploaded.', false);
      if (global.AuroraToast) {
        global.AuroraToast.success('Login ' + type + ' uploaded.', data.auditEntryId ? {
          action: { label: 'View audit entry', href: '#mod/audit/' + encodeURIComponent(data.auditEntryId) },
        } : undefined);
      }
    } catch (e) {
      setStatus(type, 'Upload failed: ' + (e && e.message ? e.message : ''), true);
    }
  }

  // Clear one asset — revert its runtime setting to empty (login reverts to the
  // theme default). Cosmetic-but-audited, like the URL save.
  async function onClear(type) {
    if (!BRANDING[type]) return;
    const r = await global.AuroraAuditedSave.run({
      heading: 'Clear login ' + type,
      body: 'Clear the login ' + type + '? The login page reverts to the theme default (no custom ' + type + ').',
      confirmLabel: 'Clear',
      cosmetic: true,
      autoRationale: 'cosmetic setting: login branding ' + type + ' cleared',
      settings: [{ key: BRANDING[type].key, value: '' }],
      successMessage: 'Login ' + type + ' cleared.',
    });
    if (r && r.saved) {
      const urlEl = document.getElementById('branding-' + type);
      if (urlEl) urlEl.value = '';
      setPreview(type, '');
      setStatus(type, '', false);
    }
  }

  // Save both branding URLs (manual URL entry) deployment-wide. Cosmetic (light
  // confirm, no typed rationale) like the theme deployment-default save (#308),
  // but still lands an audit-chain entry per setting via the auto-filled
  // rationale.
  async function saveBranding() {
    const logo = document.getElementById('branding-logo').value.trim();
    const banner = document.getElementById('branding-banner').value.trim();
    const r = await global.AuroraAuditedSave.run({
      heading: 'Save login branding',
      body: 'Update the login splash logo and banner? This is deployment-wide — every operator sees it on the login page.',
      confirmLabel: 'Save branding',
      cosmetic: true,
      autoRationale: 'cosmetic setting: login branding URLs updated',
      settings: [
        { key: BRANDING.logo.key, value: logo },
        { key: BRANDING.banner.key, value: banner },
      ],
      successMessage: 'Login branding saved.',
    });
    if (r && r.saved) {
      setPreview('logo', logo);
      setPreview('banner', banner);
    }
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
