// PaginationStrip substrate primitive (substrate primitive 10) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.10.
//
// Cursor-based pagination shared across list pages (Mod Events,
// Audit, Reports, Appeals, Accounts). State is owned by the page;
// this component renders + emits navigation events.

(function (global) {
  'use strict';

  // render({ container, prevDisabled, nextDisabled, info?, onPrev, onNext })
  //
  // XSS contract (#358 audit): the buttons are static; `spec.info` is the only
  // dynamic field and is inserted RAW. No caller passes `info` today (Sessions
  // is the sole consumer and omits it). A caller supplying `info` MUST
  // pre-escape it (or this should esc() it).
  function render(spec) {
    if (!spec || !spec.container) return;
    const c = spec.container;
    c.innerHTML =
      '<div class="pagination-strip">' +
      '  <button type="button" class="btn-secondary btn-sm pag-prev"' +
        (spec.prevDisabled ? ' disabled' : '') + '>Previous</button>' +
      '  <button type="button" class="btn-secondary btn-sm pag-next"' +
        (spec.nextDisabled ? ' disabled' : '') + '>Next</button>' +
      (spec.info ? '  <span class="pagination-info">' + spec.info + '</span>' : '') +
      '</div>';
    if (typeof spec.onPrev === 'function') {
      c.querySelector('.pag-prev').addEventListener('click', spec.onPrev);
    }
    if (typeof spec.onNext === 'function') {
      c.querySelector('.pag-next').addEventListener('click', spec.onNext);
    }
  }

  global.AuroraPagination = {
    render: render,
  };
})(window);
