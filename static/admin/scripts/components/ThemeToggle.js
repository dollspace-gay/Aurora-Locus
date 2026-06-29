// ThemeToggle substrate primitive (substrate primitive 14).
//
// v0.9 (B-themes-page §5.5.3): populated from the theme registry rather than a
// hardcoded light/dark/system triplet. The options are "Follow deployment
// default" plus each validated installed theme (§11.10.2). Two variants:
//   - 'compact': one button cycling through the options (sidebar footer)
//   - 'dropdown': a <select> (Configuration → UI & modes). A dropdown rather
//     than a radio pill so it stays single-line as the installed-theme count
//     grows (the v0.9 cohort is ten).
//
// The installed-theme list is cached in AuroraSettings (fetched at boot); this
// control subscribes so it re-paints when the list arrives.

(function (global) {
  'use strict';

  const DEFAULT_OPTION = { id: 'default', label: 'Follow deployment default' };

  function currentPref() {
    return global.AuroraSettings ? global.AuroraSettings.theme() : 'default';
  }

  function applyPref(id) {
    if (global.AuroraSettings) global.AuroraSettings.setTheme(id);
  }

  // 'Follow deployment default' + each validated installed theme. Before the
  // registry list loads, only the follow-default option shows; a subscribe
  // re-paint fills in the themes once cached.
  function themeOptions() {
    const opts = [DEFAULT_OPTION];
    const installed = (global.AuroraSettings && global.AuroraSettings.getInstalledThemes)
      ? global.AuroraSettings.getInstalledThemes() : [];
    for (const t of installed) {
      if (t && t.valid === true && t.themeId) {
        opts.push({ id: t.themeId, label: t.themeName || t.themeId });
      }
    }
    return opts;
  }

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s);
  }

  // Compact: cycle through the options. Used in the sidebar footer.
  function mountCompact(container) {
    if (!container) return;
    function paint() {
      const pref = currentPref();
      const opts = themeOptions();
      const cur = opts.find((o) => o.id === pref) || DEFAULT_OPTION;
      const icon = global.AuroraIcons ? global.AuroraIcons.render('monitor', 16) : '';
      container.innerHTML = icon + ' <span class="theme-label">' + esc(cur.label) + '</span>';
    }
    container.setAttribute('aria-label', 'Cycle theme');
    container.addEventListener('click', () => {
      const opts = themeOptions();
      const pref = currentPref();
      const idx = Math.max(0, opts.findIndex((o) => o.id === pref));
      const next = opts[(idx + 1) % opts.length];
      applyPref(next.id);
      paint();
    });
    paint();
    if (global.AuroraSettings) global.AuroraSettings.subscribe(paint);
  }

  // Dropdown: a <select> for Configuration → UI & modes. Single-line as the
  // installed-theme list grows; the operator's personal session-theme picker.
  function mountDropdown(container) {
    if (!container) return;
    function paint() {
      const pref = currentPref();
      const opts = themeOptions();
      container.innerHTML =
        '<select id="ui-theme-select" aria-label="Theme preference">' +
        opts.map((o) =>
          '<option value="' + esc(o.id) + '"' + (pref === o.id ? ' selected' : '') + '>' +
          esc(o.label) + '</option>').join('') +
        '</select>';
      const sel = container.querySelector('select');
      if (sel) {
        sel.addEventListener('change', () => {
          applyPref(sel.value);
          paint();
        });
      }
    }
    paint();
    if (global.AuroraSettings) global.AuroraSettings.subscribe(paint);
  }

  global.AuroraThemeToggle = {
    mountCompact: mountCompact,
    mountDropdown: mountDropdown,
  };
})(window);
