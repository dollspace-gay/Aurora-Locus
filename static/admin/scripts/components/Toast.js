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

  function show(message, opts) {
    const root = ensureRoot();
    opts = opts || {};
    const variant = opts.variant || 'info';
    const duration = opts.duration == null ? DEFAULT_DURATION : opts.duration;
    const id = 'toast-' + (++counter);
    const el = document.createElement('div');
    el.id = id;
    el.className = 'toast toast-' + variant;
    el.setAttribute('role', 'status');
    const safe = global.AuroraDom ? global.AuroraDom.esc(message) : String(message);
    el.innerHTML =
      '<span class="toast-message">' + safe + '</span>' +
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
