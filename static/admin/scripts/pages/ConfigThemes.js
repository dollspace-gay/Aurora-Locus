// Installed-themes section — composed into Configuration → UI & modes.
// Per docs/internal/design/v09_UI_Design.md §11.10.2 / §5.5.2.
//
// Was the standalone #configuration/themes page; folded into UI & modes (#322)
// as a section below the personal-preference controls. Exposes
// AuroraInstalledThemes.mount(container, isSuper) — it renders the row list
// (one row per theme: display name, Dark/Light pill, AAA badge on the
// high-contrast themes, the one-line description, and a per-row action) into
// the given container, with no breadcrumb/header of its own. Internal substrate
// metadata is not surfaced; a theme that fails validation keeps a distinct
// error affordance. Admin+ reads; SuperAdmin sets the deployment default
// (rationale-light cosmetic confirm, audit-logged via setRuntimeSetting
// theme.deployment-default).

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s);
  }

  let currentDefault = 'stack-classic';
  let listEl = null;
  let superFlag = false;

  // Mode + AAA are theme-intrinsic but not carried on the listInstalledThemes
  // wire shape, so they are derived here for the bundled cohort. An
  // operator-installed theme (a future release) would declare its mode in the
  // manifest; until then this set classifies the four light bundled themes.
  const LIGHT_THEMES = { light: 1, glacier: 1, meridian: 1, 'high-contrast-light': 1 };
  function themeMode(id) { return LIGHT_THEMES[id] ? 'Light' : 'Dark'; }
  function isAAA(id) { return id.indexOf('high-contrast-') === 0; }

  // Render the installed-themes row list into `container`. Called from
  // ConfigUiModes after it lays out its own controls + the section heading.
  async function mount(container, isSuper) {
    if (!container) return;
    superFlag = !!isSuper;
    container.innerHTML =
      '<div class="theme-list" data-installed-themes>' + global.AuroraSkeleton.cards(3) + '</div>';
    listEl = container.querySelector('[data-installed-themes]');
    await load();
  }

  async function load() {
    if (!listEl) return;
    let themes = [];
    try {
      const data = await global.AuroraEndpoints.ops.listInstalledThemes();
      themes = (data && Array.isArray(data.themes)) ? data.themes : [];
    } catch (e) {
      global.AuroraErrorBoundary.mount(listEl, {
        message: 'Could not load themes: ' + ((e && e.message) || 'request failed'),
        onRetry: function () { load(); },
      });
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
    if (!themes.length) { listEl.innerHTML = '<p>No themes installed.</p>'; return; }
    listEl.innerHTML = themes.map((t) => row(t, superFlag)).join('');
    wire(themes, superFlag);
  }

  function row(t, isSuper) {
    const id = t.themeId;
    const isDefault = id === currentDefault;
    const modePill = '<span class="theme-mode-pill">' + esc(themeMode(id)) + '</span>';
    const aaaPill = isAAA(id)
      ? '<span class="theme-aaa-pill" title="WCAG 2.2 AAA contrast verified">AAA</span>'
      : '';

    let action;
    if (!t.valid) {
      action = '<button type="button" class="btn-secondary btn-sm" data-errors="' + esc(id) + '">View validation errors</button>';
    } else if (isDefault) {
      action = '<span class="theme-active-pill">Active</span>';
    } else if (isSuper) {
      action = '<button type="button" class="btn-primary btn-sm" data-setdefault="' + esc(id) + '">Set as deployment default</button>';
    } else {
      action = '<span class="settings-help">SuperAdmin sets the default</span>';
    }

    // §11.7.3 discovery — only when a theme actually declares extension points
    // (none of the bundled cohort does, so the operator list stays clean) (#285).
    const points = Array.isArray(t.providedExtensionPoints) ? t.providedExtensionPoints : [];
    const extensions = points.length
      ? '<p class="theme-row-ext">Extension points: ' + points.map(esc).join(', ') + '</p>'
      : '';

    return '<div class="theme-row' + (isDefault ? ' is-active' : '') + '">'
      + '<div class="theme-row-main">'
      +   '<div class="theme-row-head">'
      +     '<span class="theme-row-name">' + esc(t.themeName || id) + '</span>'
      +     modePill + aaaPill
      +   '</div>'
      +   (t.themeDescription ? '<p class="theme-row-desc">' + esc(t.themeDescription) + '</p>' : '')
      +   extensions
      + '</div>'
      + '<div class="theme-row-action">' + action + '</div>'
      + '</div>';
  }

  function wire(themes, isSuper) {
    listEl.querySelectorAll('[data-errors]').forEach((btn) => {
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
    listEl.querySelectorAll('[data-setdefault]').forEach((btn) => {
      btn.addEventListener('click', () => setDefault(btn.dataset.setdefault, isSuper));
    });
  }

  async function setDefault(themeId, isSuper) {
    const r = await global.AuroraAuditedSave.run({
      heading: 'Set deployment-default theme',
      body: 'Make "' + themeId + '" the deployment-default theme? Every operator who has not set a personal preference will see it.',
      confirmLabel: 'Set as default',
      // #308 — theme is a cosmetic setting: a light confirm, no required
      // rationale (the change still lands an audit entry with this auto-reason).
      cosmetic: true,
      autoRationale: 'cosmetic setting: theme deployment-default = ' + themeId,
      settings: [{ key: 'theme.deployment-default', value: themeId }],
      successMessage: 'Deployment-default theme set to ' + themeId + '.',
    });
    if (r.saved) await load();
  }

  global.AuroraInstalledThemes = { mount: mount };
})(window);
