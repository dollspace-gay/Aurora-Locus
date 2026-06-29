// AuroraSpinner — task-scoped loading indicator (§8.2.5, the "action
// confirmations during a request-in-flight" tier: a Save button shows a
// spinner while its request is in flight). Not for whole-page loads — those
// use AuroraSkeleton; route transitions use the route-progress bar.
//
//   AuroraSpinner.render({ size, label })  → HTML string (a <span> spinner)
//       size:  pixel diameter (default 16). label: accessible label
//       (default localized "Loading…"); rendered as aria-label + visually-
//       hidden text so screen readers announce the in-flight state.
//   AuroraSpinner.busy(buttonEl, asyncFn)  → Promise (asyncFn's result)
//       Runs asyncFn while the button is disabled and shows an inline spinner
//       beside its label; ALWAYS restores the button (label + disabled state)
//       in a finally, even on error. The action runs regardless of spinner
//       rendering, so this is purely additive in-flight feedback — it can't
//       change whether the action succeeds.

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

  function busy(btn, asyncFn) {
    if (typeof asyncFn !== 'function') return Promise.resolve();
    if (!btn) return Promise.resolve().then(asyncFn);
    const prevHtml = btn.innerHTML;
    const prevDisabled = btn.disabled;
    btn.disabled = true;
    btn.innerHTML = render({ size: 13 }) + ' ' + prevHtml;
    return Promise.resolve()
      .then(asyncFn)
      .finally(function () {
        btn.innerHTML = prevHtml;
        btn.disabled = prevDisabled;
      });
  }

  global.AuroraSpinner = { render: render, busy: busy };
})(window);
