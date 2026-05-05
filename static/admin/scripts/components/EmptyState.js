// EmptyState substrate primitive (substrate primitive 11) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.11.
//
// Used when a list / panel / drawer has no content. Renders a Lucide
// icon + primary text + optional secondary text. The icon is the
// silent visual signal; the primary text is the message; secondary
// text is the optional next-action hint.

(function (global) {
  'use strict';

  function render(spec) {
    spec = spec || {};
    const icon = spec.icon || 'inbox';
    const primary = spec.primary || '';
    const secondary = spec.secondary || '';
    const safe1 = global.AuroraDom ? global.AuroraDom.esc(primary) : String(primary);
    const safe2 = global.AuroraDom ? global.AuroraDom.esc(secondary) : String(secondary);
    const iconSvg = global.AuroraIcons ? global.AuroraIcons.render(icon, 36) : '';
    return '<div class="empty-state" role="status" aria-live="polite">' +
           '  <div style="margin-bottom: 0.5rem; color: var(--text-tertiary);">' + iconSvg + '</div>' +
           '  <p>' + safe1 + '</p>' +
           (safe2 ? '  <p style="font-size: 0.8125rem; color: var(--text-tertiary); margin-top: 0.25rem;">' + safe2 + '</p>' : '') +
           '</div>';
  }

  global.AuroraEmptyState = {
    render: render,
  };
})(window);
