// Configuration → Themes page (route: #configuration/themes).
// Per docs/internal/design/v09_UI_Design.md §11.10.2 / §5.5.2.
//
// SuperAdmin's view of installed themes: a card per theme with its validation
// status and metadata, a per-card "Set as deployment default" action (rationale
// required, audit-logged via setRuntimeSetting theme.deployment-default), and a
// "View validation errors" affordance for themes that failed the substrate's
// validation contract (§11.10). Admin+ reads; SuperAdmin writes.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s);
  }

  let currentDefault = 'aurora-default';

  async function mount({ container }) {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Themes</nav>' +
      '<header class="page-header"><div><h2>Themes</h2><p class="page-subtitle">Installed themes, validation status, and the deployment-default selection</p></div></header>' +
      '<div id="themes-grid" class="settings-grid">' + global.AuroraSkeleton.cards(3) + '</div>';
    await load(isSuper);
    return {};
  }

  async function load(isSuper) {
    const grid = document.getElementById('themes-grid');
    if (!grid) return;
    let themes = [];
    try {
      const data = await global.AuroraEndpoints.ops.listInstalledThemes();
      themes = (data && Array.isArray(data.themes)) ? data.themes : [];
    } catch (e) {
      grid.innerHTML = '<p>Could not load themes: ' + esc(e && e.message ? e.message : 'request failed') + '</p>';
      return;
    }
    try {
      const d = await global.AuroraEndpoints.admin.getRuntimeSetting('theme.deployment-default');
      if (d && typeof d.value === 'string' && d.value.trim()) currentDefault = d.value.trim();
    } catch (e) { /* cached default stands */ }
    // Keep the global caches fresh so the picker reflects this view.
    if (global.AuroraSettings) {
      global.AuroraSettings.setInstalledThemesCache(themes);
      global.AuroraSettings.setDeploymentDefaultCache(currentDefault);
    }
    if (!themes.length) { grid.innerHTML = '<p>No themes installed.</p>'; return; }
    grid.innerHTML = themes.map((t) => card(t, isSuper)).join('');
    wire(themes, isSuper);
  }

  function card(t, isSuper) {
    const isDefault = t.themeId === currentDefault;
    const status = t.valid
      ? '<span class="badge">Valid</span>'
      : '<span class="badge badge-attention">Failed</span>';
    const defaultBadge = isDefault ? ' <span class="role-tag">Deployment default</span>' : '';
    const meta = esc(t.themeId)
      + (t.themeVersion ? ' · v' + esc(t.themeVersion) : '')
      + (t.extends ? ' · extends ' + esc(t.extends) : '')
      + ' · ' + esc(t.source);
    let actions = '';
    if (!t.valid) {
      actions = '<button type="button" class="btn-secondary btn-sm" data-errors="' + esc(t.themeId) + '">View validation errors</button>';
    } else if (isDefault) {
      actions = '<p class="settings-help">This is the current deployment default.</p>';
    } else if (isSuper) {
      actions = '<button type="button" class="btn-primary btn-sm" data-setdefault="' + esc(t.themeId) + '">Set as deployment default</button>';
    } else {
      actions = '<p class="settings-help">SuperAdmin sets the deployment default.</p>';
    }
    return '<div class="settings-card theme-card">'
      + '<h3>' + esc(t.themeName || t.themeId) + ' ' + status + defaultBadge + '</h3>'
      + '<p class="settings-help">' + meta + '</p>'
      + (t.themeDescription ? '<p>' + esc(t.themeDescription) + '</p>' : '')
      + (t.themeAuthor ? '<p class="settings-help">by ' + esc(t.themeAuthor) + '</p>' : '')
      + '<div class="theme-card-actions">' + actions + '</div>'
      + '</div>';
  }

  function wire(themes, isSuper) {
    document.querySelectorAll('[data-errors]').forEach((btn) => {
      btn.addEventListener('click', () => {
        const t = themes.find((x) => x.themeId === btn.dataset.errors);
        const errs = (t && Array.isArray(t.validationErrors)) ? t.validationErrors : [];
        const body = errs.length
          ? '<ul class="theme-validation-errors">' + errs.map((e) => '<li>' + esc(e) + '</li>').join('') + '</ul>'
          : '<p>No specific errors were recorded.</p>';
        global.AuroraModal.open({
          title: 'Validation errors — ' + (t ? (t.themeName || t.themeId) : ''),
          body: body,
        });
      });
    });
    if (!isSuper) return;
    document.querySelectorAll('[data-setdefault]').forEach((btn) => {
      btn.addEventListener('click', () => setDefault(btn.dataset.setdefault, isSuper));
    });
  }

  async function setDefault(themeId, isSuper) {
    const res = await global.AuroraModal.destructiveConfirm({
      heading: 'Set deployment-default theme',
      body: 'Make "' + themeId + '" the deployment-default theme? Every operator who has not set a personal preference will see it.',
      rationaleRequired: true,
      confirmLabel: 'Set as default',
    });
    if (!res.confirmed) return;
    try {
      const out = await global.AuroraEndpoints.admin.setRuntimeSetting({
        key: 'theme.deployment-default',
        value: themeId,
        rationale: res.rationale || '',
      });
      const auditEntryId = out && out.auditEntryId;
      global.AuroraToast.success('Deployment-default theme set to ' + themeId + '.', auditEntryId ? {
        action: { label: 'View audit entry', href: '#mod/audit/' + encodeURIComponent(auditEntryId) },
      } : undefined);
      await load(isSuper);
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configThemes', { mount: mount });
})(window);
