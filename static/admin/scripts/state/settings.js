// UI settings state — theme, locale, runtime moderation mode.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.2 + §6.14 + §11 (theming substrate).
// Theme + language live in localStorage; moderation mode comes from server
// runtime settings and is cached here.
//
// Keys:
//   ui.theme    — a named theme id ('aurora-default' | 'aurora-light' |
//                 'aurora-dark' | 'aurora-stack-classic' | …installed) OR the
//                 sentinel 'default' = "follow the deployment-default theme".
//                 (B-themes-page §11.10.2/§5.5.3.) Legacy 'light'/'dark'/
//                 'system' values are migrated on read.
//   ui.language — BCP 47 tag (default 'en')
//
// A named theme is applied by pointing the #theme-tokens / #theme-effects
// <link>s at /theme/active.css(?id=) — the server resolves the inheritance
// chain (and, for the 'default' sentinel, the deployment-default runtime
// setting). The deployment-default and installed-theme list are fetched once
// at boot (app.js) and cached here.

(function (global) {
  'use strict';

  let modMode = 'full';
  let modModeRedirect = '';
  let deploymentDefault = 'aurora-default';
  let installedThemes = [];
  const subscribers = new Set();

  // Legacy light/dark/system → named-theme migration. 'system' (OS-driven
  // light/dark) maps to the closest new affordance: follow the deployment
  // default.
  const LEGACY_THEME = { light: 'aurora-light', dark: 'aurora-dark', system: 'default' };

  function theme() {
    let v = localStorage.getItem('ui.theme') || 'default';
    if (Object.prototype.hasOwnProperty.call(LEGACY_THEME, v)) {
      v = LEGACY_THEME[v];
      try { localStorage.setItem('ui.theme', v); } catch (e) { /* localStorage disabled */ }
    }
    return v;
  }

  // The concrete theme id that the preference resolves to (the 'default'
  // sentinel resolves to the cached deployment-default).
  function resolvedThemeId() {
    const p = theme();
    return p === 'default' ? deploymentDefault : p;
  }

  function setTheme(t) {
    localStorage.setItem('ui.theme', t);
    applyTheme(t);
    notify();
  }

  // An explicit pref pins a chosen theme via ?id (which also cache-busts when
  // the pref changes). The 'default' sentinel uses NO ?id so the server
  // resolves the deployment-default — but it carries a ?v cache-bust keyed on
  // the *resolved* default, so when the deployment-default changes the URL
  // changes and the browser refetches the new theme's colors instead of
  // serving the stale cached no-id stylesheet (#306, the partial-repaint bug:
  // typography reflowed live via data-theme but active.css colors stayed
  // cached). The server ignores the extra `v` param (it keys on `id` only).
  function themeHref(base, pref) {
    if (pref === 'default') {
      return base + '?v=' + encodeURIComponent(resolvedThemeId());
    }
    return base + '?id=' + encodeURIComponent(pref);
  }

  function applyTheme(pref) {
    const tokensLink = document.getElementById('theme-tokens');
    const effectsLink = document.getElementById('theme-effects');
    const extensionsLink = document.getElementById('theme-extensions');
    if (tokensLink) tokensLink.setAttribute('href', themeHref('/theme/active.css', pref));
    if (effectsLink) effectsLink.setAttribute('href', themeHref('/theme/active-effects.css', pref));
    if (extensionsLink) extensionsLink.setAttribute('href', themeHref('/theme/active-extensions.css', pref));
    // data-theme carries the resolved id for any [data-theme="…"]-scoped hooks.
    document.documentElement.setAttribute('data-theme', resolvedThemeId());
    // Refresh the extension-point runtime cache to match the now-active theme
    // (§11.7 / #285) so themeProvidesExtension() reflects the switch. Fail-soft.
    if (global.AuroraThemeRuntime) {
      try { global.AuroraThemeRuntime.reload(pref); } catch (e) { /* non-fatal */ }
    }
  }

  function getDeploymentDefault() {
    return deploymentDefault;
  }

  function setDeploymentDefaultCache(id) {
    if (typeof id === 'string' && id.trim()) {
      deploymentDefault = id.trim();
      // Re-apply so a 'default' preference reflects the freshly-known default.
      try { applyTheme(theme()); } catch (e) { /* ignore */ }
      notify();
    }
  }

  function getInstalledThemes() {
    return installedThemes.slice();
  }

  function setInstalledThemesCache(list) {
    if (Array.isArray(list)) {
      installedThemes = list;
      notify();
    }
  }

  function language() {
    return localStorage.getItem('ui.language')
      || (navigator.language || 'en').split('-')[0]
      || 'en';
  }

  function setLanguage(lang) {
    localStorage.setItem('ui.language', lang);
    notify();
    // Per §5.5.2 the page reloads to apply locale changes.
    window.location.reload();
  }

  function getModerationMode() {
    return modMode;
  }

  function getModerationModeRedirect() {
    return modModeRedirect;
  }

  function setModerationModeCache(mode, redirect) {
    modMode = mode;
    if (typeof redirect === 'string') modModeRedirect = redirect;
    notify();
  }

  function subscribe(fn) {
    subscribers.add(fn);
    return () => subscribers.delete(fn);
  }

  function notify() {
    for (const fn of subscribers) {
      try {
        fn({
          theme: theme(),
          language: language(),
          modMode: modMode,
          modModeRedirect: modModeRedirect,
          deploymentDefault: deploymentDefault,
          installedThemes: installedThemes,
        });
      } catch (e) { /* ignore */ }
    }
  }

  // Apply theme on module load so initial paint matches preference. For the
  // 'default' sentinel this leaves the server-resolved no-id <link> in place
  // (no flash); a concrete preference re-points the links here.
  try { applyTheme(theme()); } catch (e) { /* localStorage disabled */ }

  global.AuroraSettings = {
    theme: theme,
    setTheme: setTheme,
    applyTheme: applyTheme,
    resolvedThemeId: resolvedThemeId,
    getDeploymentDefault: getDeploymentDefault,
    setDeploymentDefaultCache: setDeploymentDefaultCache,
    getInstalledThemes: getInstalledThemes,
    setInstalledThemesCache: setInstalledThemesCache,
    language: language,
    setLanguage: setLanguage,
    getModerationMode: getModerationMode,
    getModerationModeRedirect: getModerationModeRedirect,
    setModerationModeCache: setModerationModeCache,
    subscribe: subscribe,
  };
})(window);
