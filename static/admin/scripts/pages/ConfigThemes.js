// Configuration → Themes page (route: #configuration/themes).
// Per docs/internal/design/v09_UI_Design.md §11.10.2 / §5.5.2.
//
// SuperAdmin's view of installed themes as a single-column row list: one row
// per theme showing display name, a Dark/Light mode pill, an AAA badge on the
// high-contrast themes, the one-line themeDescription, and a per-row action
// ("Set as deployment default", or an "Active" pill if it is the current
// default). The active row carries a left-border accent so the default is
// obvious without reading the action column. Internal substrate metadata
// (validation state on the happy path, slug/version/inheritance/source, author
// attribution) is not surfaced — the picker is operator-facing. A theme that
// fails validation keeps a distinct error affordance. Admin+ reads; SuperAdmin
// writes (rationale-light cosmetic confirm, audit-logged via setRuntimeSetting
// theme.deployment-default).

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s);
  }

  let currentDefault = 'stack-classic';

  // Mode + AAA are theme-intrinsic but not carried on the listInstalledThemes
  // wire shape, so they are derived here for the bundled cohort. An
  // operator-installed theme (a future release) would declare its mode in the
  // manifest; until then this set classifies the four light bundled themes.
  const LIGHT_THEMES = { light: 1, glacier: 1, meridian: 1, 'high-contrast-light': 1 };
  function themeMode(id) { return LIGHT_THEMES[id] ? 'Light' : 'Dark'; }
  function isAAA(id) { return id.indexOf('high-contrast-') === 0; }

  async function mount({ container }) {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Themes</nav>' +
      '<header class="page-header"><div><h2>Themes</h2><p class="page-subtitle">Installed themes and the deployment-default selection</p></div></header>' +
      '<div id="themes-list" class="theme-list">' + global.AuroraSkeleton.cards(3) + '</div>';
    await load(isSuper);
    return {};
  }

  async function load(isSuper) {
    const list = document.getElementById('themes-list');
    if (!list) return;
    let themes = [];
    try {
      const data = await global.AuroraEndpoints.ops.listInstalledThemes();
      themes = (data && Array.isArray(data.themes)) ? data.themes : [];
    } catch (e) {
      global.AuroraErrorBoundary.mount(list, {
        message: 'Could not load themes: ' + ((e && e.message) || 'request failed'),
        onRetry: function () { load(isSuper); },
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
    if (!themes.length) { list.innerHTML = '<p>No themes installed.</p>'; return; }
    list.innerHTML = themes.map((t) => row(t, isSuper)).join('');
    wire(themes, isSuper);
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
    if (r.saved) await load(isSuper);
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configThemes', { mount: mount });
})(window);
