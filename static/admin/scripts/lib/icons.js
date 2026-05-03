// Lucide icon set (substrate primitive 14) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.13.
//
// Inline SVG icons (no external dependency, no CDN). Each icon is a
// function returning the SVG markup at the requested size. Vanilla JS
// IIFE; exposes window.AuroraIcons.
//
// Curated subset matching the v0.2 UI's actual usage. Default size
// 16px; sidebar nav uses 20px. Color inherits via currentColor.
// Decorative icons add aria-hidden="true"; icon-only buttons attach
// their own aria-label on the wrapping <button>.
//
// Total footprint ~20KB inline; acceptable per §6.13.
//
// Source paths come from lucide.dev (MIT-licensed). Stroke-width 1.5
// matches Lucide default. Each icon's stroke/fill is currentColor.

(function (global) {
  'use strict';

  function svg(size, body) {
    const sz = size || 16;
    return '<svg xmlns="http://www.w3.org/2000/svg" width="' + sz +
           '" height="' + sz + '" viewBox="0 0 24 24" fill="none" ' +
           'stroke="currentColor" stroke-width="1.5" stroke-linecap="round" ' +
           'stroke-linejoin="round" aria-hidden="true">' + body + '</svg>';
  }

  // Each icon function: (size?: number) => string
  // Body strings are the inner SVG paths from Lucide (lucide.dev),
  // copied here verbatim with stroke/fill attributes hoisted to the
  // wrapping <svg>.
  const icons = {
    // Navigation icons
    'layout-dashboard': (s) => svg(s,
      '<rect width="7" height="9" x="3" y="3" rx="1"/>' +
      '<rect width="7" height="5" x="14" y="3" rx="1"/>' +
      '<rect width="7" height="9" x="14" y="12" rx="1"/>' +
      '<rect width="7" height="5" x="3" y="16" rx="1"/>'),
    'gavel': (s) => svg(s,
      '<path d="m14.5 12.5-8 8a2.119 2.119 0 1 1-3-3l8-8"/>' +
      '<path d="m16 16 6-6"/>' +
      '<path d="m8 8 6-6"/>' +
      '<path d="m9 7 8 8"/>' +
      '<path d="m21 11-8-8"/>'),
    'file-text': (s) => svg(s,
      '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/>' +
      '<path d="M14 2v4a2 2 0 0 0 2 2h4"/>' +
      '<path d="M10 9H8"/>' +
      '<path d="M16 13H8"/>' +
      '<path d="M16 17H8"/>'),
    'scale': (s) => svg(s,
      '<path d="m16 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z"/>' +
      '<path d="m2 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z"/>' +
      '<path d="M7 21h10"/>' +
      '<path d="M12 3v18"/>' +
      '<path d="M3 7h2c2 0 5-1 7-2 2 1 5 2 7 2h2"/>'),
    'shield-alert': (s) => svg(s,
      '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>' +
      '<path d="M12 8v4"/>' +
      '<path d="M12 16h.01"/>'),
    'archive': (s) => svg(s,
      '<rect width="20" height="5" x="2" y="3" rx="1"/>' +
      '<path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/>' +
      '<path d="M10 12h4"/>'),
    'users': (s) => svg(s,
      '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/>' +
      '<circle cx="9" cy="7" r="4"/>' +
      '<path d="M22 21v-2a4 4 0 0 0-3-3.87"/>' +
      '<path d="M16 3.13a4 4 0 0 1 0 7.75"/>'),
    'ticket': (s) => svg(s,
      '<path d="M2 9a3 3 0 0 1 0 6v2a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-2a3 3 0 0 1 0-6V7a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2Z"/>' +
      '<path d="M13 5v2"/>' +
      '<path d="M13 17v2"/>' +
      '<path d="M13 11v2"/>'),
    'activity': (s) => svg(s,
      '<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.5.5 0 0 1-.96 0L9.24 2.18a.5.5 0 0 0-.96 0l-2.35 8.36A2 2 0 0 1 4 12H2"/>'),
    'network': (s) => svg(s,
      '<rect x="16" y="16" width="6" height="6" rx="1"/>' +
      '<rect x="2" y="16" width="6" height="6" rx="1"/>' +
      '<rect x="9" y="2" width="6" height="6" rx="1"/>' +
      '<path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/>' +
      '<path d="M12 12V8"/>'),
    'image': (s) => svg(s,
      '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/>' +
      '<circle cx="9" cy="9" r="2"/>' +
      '<path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>'),
    'gauge': (s) => svg(s,
      '<path d="m12 14 4-4"/>' +
      '<path d="M3.34 19a10 10 0 1 1 17.32 0"/>'),
    'heart-pulse': (s) => svg(s,
      '<path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/>' +
      '<path d="M3.22 12H9.5l.5-1 2 4.5 2-7 1.5 3.5h5.27"/>'),
    'server': (s) => svg(s,
      '<rect width="20" height="8" x="2" y="2" rx="2" ry="2"/>' +
      '<rect width="20" height="8" x="2" y="14" rx="2" ry="2"/>' +
      '<line x1="6" x2="6.01" y1="6" y2="6"/>' +
      '<line x1="6" x2="6.01" y1="18" y2="18"/>'),
    'settings': (s) => svg(s,
      '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>' +
      '<circle cx="12" cy="12" r="3"/>'),
    'sliders': (s) => svg(s,
      '<line x1="4" x2="4" y1="21" y2="14"/>' +
      '<line x1="4" x2="4" y1="10" y2="3"/>' +
      '<line x1="12" x2="12" y1="21" y2="12"/>' +
      '<line x1="12" x2="12" y1="8" y2="3"/>' +
      '<line x1="20" x2="20" y1="21" y2="16"/>' +
      '<line x1="20" x2="20" y1="12" y2="3"/>' +
      '<line x1="2" x2="6" y1="14" y2="14"/>' +
      '<line x1="10" x2="14" y1="8" y2="8"/>' +
      '<line x1="18" x2="22" y1="16" y2="16"/>'),
    'key': (s) => svg(s,
      '<circle cx="7.5" cy="15.5" r="5.5"/>' +
      '<path d="m21 2-9.6 9.6"/>' +
      '<path d="m15.5 7.5 3 3L22 7l-3-3"/>'),
    'plug': (s) => svg(s,
      '<path d="M12 22v-5"/>' +
      '<path d="M9 7V2"/>' +
      '<path d="M15 7V2"/>' +
      '<path d="M6 13V8h12v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4Z"/>'),

    // Action icons
    'ban': (s) => svg(s,
      '<circle cx="12" cy="12" r="10"/>' +
      '<path d="m4.9 4.9 14.2 14.2"/>'),
    'pause': (s) => svg(s,
      '<rect x="14" y="4" width="4" height="16" rx="1"/>' +
      '<rect x="6" y="4" width="4" height="16" rx="1"/>'),
    'play': (s) => svg(s,
      '<polygon points="6 3 20 12 6 21 6 3"/>'),
    'tag': (s) => svg(s,
      '<path d="M12.586 2.586A2 2 0 0 0 11.172 2H4a2 2 0 0 0-2 2v7.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l6.58-6.58a2.426 2.426 0 0 0 0-3.42z"/>' +
      '<circle cx="7.5" cy="7.5" r=".5" fill="currentColor"/>'),
    'tag-x': (s) => svg(s,
      '<path d="M9 5H4a2 2 0 0 0-2 2v3.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l3.585-3.585"/>' +
      '<path d="m17 4 5 5"/>' +
      '<path d="m22 4-5 5"/>' +
      '<circle cx="7.5" cy="7.5" r=".5" fill="currentColor"/>'),
    'mail': (s) => svg(s,
      '<rect width="20" height="16" x="2" y="4" rx="2"/>' +
      '<path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/>'),
    'trash-2': (s) => svg(s,
      '<path d="M3 6h18"/>' +
      '<path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/>' +
      '<path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>' +
      '<line x1="10" x2="10" y1="11" y2="17"/>' +
      '<line x1="14" x2="14" y1="11" y2="17"/>'),
    'refresh-cw': (s) => svg(s,
      '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/>' +
      '<path d="M21 3v5h-5"/>' +
      '<path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/>' +
      '<path d="M3 21v-5h5"/>'),
    'download': (s) => svg(s,
      '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>' +
      '<polyline points="7 10 12 15 17 10"/>' +
      '<line x1="12" x2="12" y1="15" y2="3"/>'),
    'upload': (s) => svg(s,
      '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>' +
      '<polyline points="17 8 12 3 7 8"/>' +
      '<line x1="12" x2="12" y1="3" y2="15"/>'),
    'copy': (s) => svg(s,
      '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>' +
      '<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>'),
    'external-link': (s) => svg(s,
      '<path d="M15 3h6v6"/>' +
      '<path d="M10 14 21 3"/>' +
      '<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>'),
    'plus': (s) => svg(s,
      '<path d="M5 12h14"/>' +
      '<path d="M12 5v14"/>'),
    'minus': (s) => svg(s,
      '<path d="M5 12h14"/>'),
    'x': (s) => svg(s,
      '<path d="M18 6 6 18"/>' +
      '<path d="m6 6 12 12"/>'),
    'check': (s) => svg(s,
      '<path d="M20 6 9 17l-5-5"/>'),
    'alert-triangle': (s) => svg(s,
      '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z"/>' +
      '<path d="M12 9v4"/>' +
      '<path d="M12 17h.01"/>'),
    'info': (s) => svg(s,
      '<circle cx="12" cy="12" r="10"/>' +
      '<path d="M12 16v-4"/>' +
      '<path d="M12 8h.01"/>'),
    'eye': (s) => svg(s,
      '<path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z"/>' +
      '<circle cx="12" cy="12" r="3"/>'),
    'eye-off': (s) => svg(s,
      '<path d="M9.88 9.88a3 3 0 1 0 4.24 4.24"/>' +
      '<path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68"/>' +
      '<path d="M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61"/>' +
      '<line x1="2" x2="22" y1="2" y2="22"/>'),

    // Filter / UI icons
    'search': (s) => svg(s,
      '<circle cx="11" cy="11" r="8"/>' +
      '<path d="m21 21-4.3-4.3"/>'),
    'calendar': (s) => svg(s,
      '<rect width="18" height="18" x="3" y="4" rx="2" ry="2"/>' +
      '<line x1="16" x2="16" y1="2" y2="6"/>' +
      '<line x1="8" x2="8" y1="2" y2="6"/>' +
      '<line x1="3" x2="21" y1="10" y2="10"/>'),
    'chevron-down': (s) => svg(s, '<path d="m6 9 6 6 6-6"/>'),
    'chevron-right': (s) => svg(s, '<path d="m9 18 6-6-6-6"/>'),
    'chevron-left': (s) => svg(s, '<path d="m15 18-6-6 6-6"/>'),
    'chevron-up': (s) => svg(s, '<path d="m18 15-6-6-6 6"/>'),
    'arrow-up': (s) => svg(s, '<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>'),
    'arrow-down': (s) => svg(s, '<path d="M12 5v14"/><path d="m19 12-7 7-7-7"/>'),
    'more-horizontal': (s) => svg(s,
      '<circle cx="12" cy="12" r="1"/>' +
      '<circle cx="19" cy="12" r="1"/>' +
      '<circle cx="5" cy="12" r="1"/>'),
    'filter': (s) => svg(s,
      '<polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/>'),
    'command': (s) => svg(s,
      '<path d="M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3"/>'),
    'sun': (s) => svg(s,
      '<circle cx="12" cy="12" r="4"/>' +
      '<path d="M12 2v2"/>' +
      '<path d="M12 20v2"/>' +
      '<path d="m4.93 4.93 1.41 1.41"/>' +
      '<path d="m17.66 17.66 1.41 1.41"/>' +
      '<path d="M2 12h2"/>' +
      '<path d="M20 12h2"/>' +
      '<path d="m6.34 17.66-1.41 1.41"/>' +
      '<path d="m19.07 4.93-1.41 1.41"/>'),
    'moon': (s) => svg(s,
      '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>'),
    'monitor': (s) => svg(s,
      '<rect width="20" height="14" x="2" y="3" rx="2"/>' +
      '<line x1="8" x2="16" y1="21" y2="21"/>' +
      '<line x1="12" x2="12" y1="17" y2="21"/>'),
    'dot': (s) => svg(s, '<circle cx="12" cy="12" r="1"/>'),
    'circle': (s) => svg(s, '<circle cx="12" cy="12" r="10"/>'),
    'square': (s) => svg(s, '<rect width="18" height="18" x="3" y="3" rx="2"/>'),
    'log-out': (s) => svg(s,
      '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/>' +
      '<polyline points="16 17 21 12 16 7"/>' +
      '<line x1="21" x2="9" y1="12" y2="12"/>'),

    // Status / feedback icons
    'check-circle': (s) => svg(s,
      '<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/>' +
      '<polyline points="22 4 12 14.01 9 11.01"/>'),
    'x-circle': (s) => svg(s,
      '<circle cx="12" cy="12" r="10"/>' +
      '<path d="m15 9-6 6"/>' +
      '<path d="m9 9 6 6"/>'),
    'alert-circle': (s) => svg(s,
      '<circle cx="12" cy="12" r="10"/>' +
      '<line x1="12" x2="12" y1="8" y2="12"/>' +
      '<line x1="12" x2="12.01" y1="16" y2="16"/>'),
    'alert-octagon': (s) => svg(s,
      '<polygon points="7.86 2 16.14 2 22 7.86 22 16.14 16.14 22 7.86 22 2 16.14 2 7.86 7.86 2"/>' +
      '<line x1="12" x2="12" y1="8" y2="12"/>' +
      '<line x1="12" x2="12.01" y1="16" y2="16"/>'),
    'clock': (s) => svg(s,
      '<circle cx="12" cy="12" r="10"/>' +
      '<polyline points="12 6 12 12 16 14"/>'),
    'loader-2': (s) => svg(s,
      '<path d="M21 12a9 9 0 1 1-6.219-8.56"/>'),
    'shield-check': (s) => svg(s,
      '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>' +
      '<path d="m9 12 2 2 4-4"/>'),
    'inbox': (s) => svg(s,
      '<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/>' +
      '<path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z"/>'),
  };

  // Render an icon by name. Falls back to a question-mark dot if the
  // requested icon is not in the curated set; logs a warning so missing
  // icons surface during development.
  function render(name, size) {
    const fn = icons[name];
    if (!fn) {
      // Fall through to a small dot so layout doesn't shift.
      return svg(size, '<circle cx="12" cy="12" r="10" stroke-dasharray="2"/>' +
                       '<text x="12" y="16" text-anchor="middle" font-size="10" fill="currentColor" stroke="none">?</text>');
    }
    return fn(size);
  }

  function inject(name, container, size) {
    if (!container) return;
    container.innerHTML = render(name, size);
  }

  global.AuroraIcons = {
    render: render,
    inject: inject,
    list: () => Object.keys(icons),
  };
})(window);
