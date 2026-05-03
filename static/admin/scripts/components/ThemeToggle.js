// ThemeToggle substrate primitive (substrate primitive 14) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.14.
//
// Three-state toggle for Light / Dark / System. Two variants:
//   - 'compact': icon-only segmented pill (sidebar footer)
//   - 'full': labeled radios (Settings → UI & modes)

(function (global) {
  'use strict';

  function applyTheme(t) {
    if (global.AuroraSettings) global.AuroraSettings.setTheme(t);
  }

  function currentTheme() {
    return global.AuroraSettings ? global.AuroraSettings.theme() : (localStorage.getItem('ui.theme') || 'system');
  }

  function iconFor(t) {
    if (!global.AuroraIcons) return '';
    if (t === 'light') return global.AuroraIcons.render('sun', 16);
    if (t === 'dark') return global.AuroraIcons.render('moon', 16);
    return global.AuroraIcons.render('monitor', 16);
  }

  // Compact: one button cycling through light → dark → system. Used
  // in sidebar footer where horizontal space is constrained.
  function mountCompact(container) {
    if (!container) return;
    function paint() {
      const t = currentTheme();
      container.innerHTML = iconFor(t) + ' <span class="theme-label">' +
        (t === 'light' ? 'Light' : t === 'dark' ? 'Dark' : 'System') + '</span>';
    }
    container.setAttribute('aria-label', 'Toggle theme');
    container.addEventListener('click', () => {
      const t = currentTheme();
      const next = t === 'light' ? 'dark' : t === 'dark' ? 'system' : 'light';
      applyTheme(next);
      paint();
    });
    paint();
    if (global.AuroraSettings) global.AuroraSettings.subscribe(paint);
  }

  // Full: three-segment role=radiogroup pill for Settings → UI & modes.
  function mountFull(container) {
    if (!container) return;
    function paint() {
      const t = currentTheme();
      const seg = (val, lbl) =>
        '<button type="button" role="radio" aria-checked="' + (t === val ? 'true' : 'false') +
        '" data-theme="' + val + '">' + iconFor(val) + ' ' + lbl + '</button>';
      container.innerHTML = '<div class="theme-toggle-pill" role="radiogroup" aria-label="Theme preference">' +
        seg('light', 'Light') + seg('dark', 'Dark') + seg('system', 'System') + '</div>';
      container.querySelectorAll('button[role="radio"]').forEach((btn) => {
        btn.addEventListener('click', () => {
          applyTheme(btn.dataset.theme);
          paint();
        });
        btn.addEventListener('keydown', (e) => {
          if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
          const all = Array.from(container.querySelectorAll('button[role="radio"]'));
          const idx = all.indexOf(btn);
          const next = e.key === 'ArrowRight' ? (idx + 1) % all.length : (idx - 1 + all.length) % all.length;
          all[next].focus();
          all[next].click();
        });
      });
    }
    paint();
    if (global.AuroraSettings) global.AuroraSettings.subscribe(paint);
  }

  global.AuroraThemeToggle = {
    mountCompact: mountCompact,
    mountFull: mountFull,
  };
})(window);
