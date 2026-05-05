// Drawer substrate primitive (substrate primitive 8) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.8.
//
// Collapsible content surface used on detail pages. Wraps native
// <details>/<summary> for keyboard accessibility but adds chevron icon
// and theming. open/closed state persists in sessionStorage keyed by
// id so navigating back returns the operator to the same drawer state.

(function (global) {
  'use strict';

  function escId(s) {
    return String(s || '').replace(/[^a-zA-Z0-9_-]/g, '_');
  }

  function chev() {
    if (global.AuroraIcons) return global.AuroraIcons.render('chevron-right', 16);
    return '<span aria-hidden="true">▶</span>';
  }

  // Render a drawer. Accepts:
  //   id: stable id (used for sessionStorage key)
  //   summary: header HTML/text (string)
  //   bodyHtml: HTML string for the body
  //   open: default open state (true/false)
  //   roleTag?: optional badge text after summary (e.g. "Mod+", "Admin+")
  function render(spec) {
    spec = spec || {};
    const id = 'drawer-' + escId(spec.id || ('d' + Math.random().toString(36).slice(2, 8)));
    const stored = sessionStorage.getItem('drawer.' + id);
    const open = stored != null ? stored === '1' : !!spec.open;
    const tag = spec.roleTag
      ? ' <span class="role-tag">' + (global.AuroraDom ? global.AuroraDom.esc(spec.roleTag) : spec.roleTag) + '</span>'
      : '';
    return '<details id="' + id + '" class="drawer"' + (open ? ' open' : '') + '>' +
           '  <summary class="drawer-summary">' +
           '    <span class="drawer-summary-text">' + (spec.summary || '') + tag + '</span>' +
           '    <span class="chev">' + chev() + '</span>' +
           '  </summary>' +
           '  <div class="drawer-body">' + (spec.bodyHtml || '') + '</div>' +
           '</details>';
  }

  // Wire persistence handler — call after mount to track open/closed.
  function attach(rootEl) {
    if (!rootEl) return;
    const drawers = rootEl.querySelectorAll('details.drawer');
    drawers.forEach((el) => {
      el.addEventListener('toggle', () => {
        sessionStorage.setItem('drawer.' + el.id, el.open ? '1' : '0');
      });
    });
  }

  global.AuroraDrawer = {
    render: render,
    attach: attach,
  };
})(window);
