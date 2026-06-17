// AuroraSpinner — task-scoped loading indicator (§8.2.5, the "action
// confirmations during a request-in-flight" tier: a Save button shows a
// spinner while its request is in flight). Not for whole-page loads — those
// use AuroraSkeleton; route transitions use the route-progress bar.
//
//   AuroraSpinner.render({ size, label })  → HTML string (a <span> spinner)
//       size:  pixel diameter (default 16). label: accessible label
//       (default localized "Loading…"); rendered as aria-label + visually-
//       hidden text so screen readers announce the in-flight state.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  function render(spec) {
    spec = spec || {};
    const size = Number(spec.size) > 0 ? Number(spec.size) : 16;
    const label = spec.label || (global.t ? global.t('common.loading') : 'Loading…');
    const style = 'width:' + size + 'px;height:' + size + 'px';
    return (
      '<span class="spinner" role="status" aria-label="' + esc(label) + '" style="' + style + '">' +
      '<span class="sr-only">' + esc(label) + '</span>' +
      '</span>'
    );
  }

  global.AuroraSpinner = { render: render };
})(window);
