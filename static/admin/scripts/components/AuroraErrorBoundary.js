// AuroraErrorBoundary — render-blocking error fallback (§8.2.4, the
// "top-level boundary" tier). For failures where the page (or a whole
// section) can't render meaningfully: a malformed response, a primary-data
// fetch that failed, auth-required-but-missing. Takes over the region with a
// titled message + optional retry + optional collapsible detail.
//
//   AuroraErrorBoundary.render({ title, message, retryLabel, detail })  → HTML
//   AuroraErrorBoundary.mount(host, { title, message, onRetry, retryLabel, detail })
//       Sets host's content to the boundary and wires retry when onRetry is
//       given. The path for "page-level fetch failed → the page can't render".

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  function defaultRetryLabel() {
    return global.t ? global.t('common.retry') : 'Retry';
  }

  function render(spec) {
    spec = spec || {};
    const title = spec.title || (global.t ? global.t('errors.generic') : 'Something went wrong.');
    const message = spec.message || '';
    const icon = global.AuroraIcons ? global.AuroraIcons.render('alert-triangle', 36) : '';
    const retry = spec.retryLabel
      ? '<button type="button" class="btn btn-primary error-boundary-retry" data-aurora-retry>' +
        esc(spec.retryLabel) + '</button>'
      : '';
    const detail = spec.detail
      ? '<details class="error-boundary-detail"><summary>Details</summary><pre>' +
        esc(spec.detail) + '</pre></details>'
      : '';
    return (
      '<div class="error-boundary" role="alert">' +
      '<div class="error-boundary-icon" aria-hidden="true">' + icon + '</div>' +
      '<h3 class="error-boundary-title">' + esc(title) + '</h3>' +
      (message ? '<p class="error-boundary-msg">' + esc(message) + '</p>' : '') +
      retry +
      detail +
      '</div>'
    );
  }

  function mount(host, spec) {
    if (!host) return host;
    spec = spec || {};
    const retryLabel = spec.onRetry ? (spec.retryLabel || defaultRetryLabel()) : null;
    host.innerHTML = render({
      title: spec.title, message: spec.message, retryLabel: retryLabel, detail: spec.detail,
    });
    if (spec.onRetry) {
      const btn = host.querySelector('[data-aurora-retry]');
      if (btn) btn.addEventListener('click', spec.onRetry);
    }
    return host;
  }

  global.AuroraErrorBoundary = { render: render, mount: mount };
})(window);
