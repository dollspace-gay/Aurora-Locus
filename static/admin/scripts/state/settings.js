// UI settings state — theme, locale, runtime moderation mode.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.5.2 + §6.14 + §11.3.
// Theme + language live in localStorage; moderation mode comes from
// server runtime settings and is cached here.
//
// Keys:
//   ui.theme    — 'light' | 'dark' | 'system'  (preserved key for back-compat)
//   ui.language — BCP 47 tag (default 'en')
// Moderation mode is read from tools.aurora.admin.getRuntimeSetting and
// surfaced via getModerationMode().

(function (global) {
  'use strict';

  let modMode = 'full';
  let modModeRedirect = '';
  const subscribers = new Set();

  function theme() {
    return localStorage.getItem('ui.theme') || 'system';
  }

  function setTheme(t) {
    localStorage.setItem('ui.theme', t);
    applyTheme(t);
    notify();
  }

  function applyTheme(t) {
    const resolved = (t === 'system')
      ? (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
      : t;
    document.documentElement.setAttribute('data-theme', resolved);
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
      try { fn({ theme: theme(), language: language(), modMode: modMode, modModeRedirect: modModeRedirect }); }
      catch (e) { /* ignore */ }
    }
  }

  // Apply theme on module load so initial paint matches preference.
  try { applyTheme(theme()); } catch (e) { /* localStorage disabled */ }

  // Listen to system theme changes when in 'system' mode.
  if (window.matchMedia) {
    try {
      const mq = window.matchMedia('(prefers-color-scheme: dark)');
      const onChange = () => { if (theme() === 'system') applyTheme('system'); };
      if (typeof mq.addEventListener === 'function') mq.addEventListener('change', onChange);
      else if (typeof mq.addListener === 'function') mq.addListener(onChange);
    } catch (e) { /* ignore */ }
  }

  global.AuroraSettings = {
    theme: theme,
    setTheme: setTheme,
    applyTheme: applyTheme,
    language: language,
    setLanguage: setLanguage,
    getModerationMode: getModerationMode,
    getModerationModeRedirect: getModerationModeRedirect,
    setModerationModeCache: setModerationModeCache,
    subscribe: subscribe,
  };
})(window);
