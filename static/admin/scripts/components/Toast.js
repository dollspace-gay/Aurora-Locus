// Toast notification substrate (substrate primitive 7) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.7.
//
// Single toast root #toast-root mounted next to <body>. Toasts stack
// bottom-right and auto-dismiss after 5s by default. Aria-live polite
// region announces messages to screen readers.

(function (global) {
  'use strict';

  const DEFAULT_DURATION = 5000;
  let counter = 0;

  function ensureRoot() {
    let root = document.getElementById('toast-root');
    if (!root) {
      root = document.createElement('div');
      root.id = 'toast-root';
      root.setAttribute('aria-live', 'polite');
      root.setAttribute('aria-atomic', 'true');
      document.body.appendChild(root);
    }
    return root;
  }

  // Reject hrefs that could execute JavaScript or escape the SPA. Only
  // same-origin hash routes (#...) and same-origin paths (/...) are
  // allowed; javascript:, data:, vbscript:, http(s):, etc. are
  // rejected so an upstream caller can't construct a phishing toast.
  function isSafeActionHref(href) {
    if (typeof href !== 'string') return false;
    return href.charAt(0) === '#' || href.charAt(0) === '/';
  }

  function show(message, opts) {
    const root = ensureRoot();
    opts = opts || {};
    const variant = opts.variant || 'info';
    const duration = opts.duration == null ? DEFAULT_DURATION : opts.duration;
    const action = opts.action;
    const id = 'toast-' + (++counter);
    const el = document.createElement('div');
    el.id = id;
    el.className = 'toast toast-' + variant;
    el.setAttribute('role', 'status');
    const esc = global.AuroraDom ? global.AuroraDom.esc : (s) => String(s == null ? '' : s);
    const safe = esc(message);
    // Optional inline action link (e.g., "View audit entry"). When
    // present, renders between the message and the close button as
    // an anchor the browser dispatches via the existing hash router.
    // Per V04_DESIGN §5.4.3 sub-3e. Defensive: only render when both
    // label + href are present and href is same-origin (see
    // isSafeActionHref). Missing or malformed action silently degrades
    // to a no-action toast.
    let actionHtml = '';
    if (action
        && typeof action.label === 'string'
        && action.label.length > 0
        && isSafeActionHref(action.href)) {
      actionHtml = '<a class="toast-action" href="' + esc(action.href) +
                   '">' + esc(action.label) + '</a>';
    }
    el.innerHTML =
      '<span class="toast-message">' + safe + '</span>' +
      actionHtml +
      '<button class="toast-close" aria-label="Dismiss">×</button>';
    el.querySelector('.toast-close').addEventListener('click', () => dismiss(id));
    root.appendChild(el);
    if (duration > 0) {
      setTimeout(() => dismiss(id), duration);
    }
    return id;
  }

  function dismiss(id) {
    const el = document.getElementById(id);
    if (el && el.parentNode) el.parentNode.removeChild(el);
  }

  global.AuroraToast = {
    show: show,
    info: (m, o) => show(m, Object.assign({ variant: 'info' }, o || {})),
    success: (m, o) => show(m, Object.assign({ variant: 'success' }, o || {})),
    warning: (m, o) => show(m, Object.assign({ variant: 'warning' }, o || {})),
    danger: (m, o) => show(m, Object.assign({ variant: 'danger' }, o || {})),
    dismiss: dismiss,
  };
})(window);
