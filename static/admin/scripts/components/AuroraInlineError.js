// AuroraInlineError — contextual inline error with a retry affordance
// (§8.2.4, the "inline error with retry" tier). For form-shaped pages,
// settings saves, and content fetches whose failure is contextual to the
// operator's current task: the error renders in place, with a Retry button
// next to it. (Background-poll failures use AuroraToast; render-blocking
// failures use AuroraErrorBoundary.)
//
//   AuroraInlineError.render({ message, retryLabel })  → HTML string
//       A standalone inline-error block; if retryLabel is set it includes a
//       retry button tagged [data-aurora-retry] for the caller to wire.
//   AuroraInlineError.mount(host, { message, onRetry, retryLabel })  → host
//       Sets host's content to the rendered error and, when onRetry is given,
//       wires the retry button to it. The ergonomic path for "fetch failed →
//       show error + retry re-fetches".

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
    const message = spec.message || (global.t ? global.t('errors.generic') : 'Something went wrong.');
    const icon = global.AuroraIcons ? global.AuroraIcons.render('alert-circle', 18) : '';
    const retry = spec.retryLabel
      ? '<button type="button" class="btn btn-secondary inline-error-retry" data-aurora-retry>' +
        esc(spec.retryLabel) + '</button>'
      : '';
    return (
      '<div class="inline-error" role="alert">' +
      '<span class="inline-error-icon" aria-hidden="true">' + icon + '</span>' +
      '<span class="inline-error-msg">' + esc(message) + '</span>' +
      retry +
      '</div>'
    );
  }

  function mount(host, spec) {
    if (!host) return host;
    spec = spec || {};
    const retryLabel = spec.onRetry ? (spec.retryLabel || defaultRetryLabel()) : null;
    host.innerHTML = render({ message: spec.message, retryLabel: retryLabel });
    if (spec.onRetry) {
      const btn = host.querySelector('[data-aurora-retry]');
      if (btn) btn.addEventListener('click', spec.onRetry);
    }
    return host;
  }

  global.AuroraInlineError = { render: render, mount: mount };
})(window);
