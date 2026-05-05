// Modal substrate primitive.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.6 + §7.10. A single modal
// surface mounts into #modal-root with focus trap, Esc-to-close, and
// overlay click handling. Pages don't manipulate DOM directly — they
// open a modal via openModal(spec) and close it via the returned handle
// or AuroraModal.close().
//
// API:
//   const handle = AuroraModal.open({
//     title: 'Action',
//     body: htmlString | Node,
//     footer?: htmlString | Node,
//     dismissible?: bool (default true),
//     onClose?: () => void,
//   });
//   handle.close();
//   AuroraModal.close();   // close current

(function (global) {
  'use strict';

  let activeHandle = null;
  let releaseTrap = null;

  function open(spec) {
    if (activeHandle) close();
    spec = spec || {};
    const root = ensureRoot();
    const modal = document.createElement('div');
    modal.className = 'modal active';
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    modal.setAttribute('aria-labelledby', 'modal-title-' + Date.now());

    const titleId = modal.getAttribute('aria-labelledby');
    const title = spec.title || '';
    const bodyHtml = (typeof spec.body === 'string') ? spec.body : '';
    const footerHtml = (typeof spec.footer === 'string') ? spec.footer : '';

    modal.innerHTML =
      '<div class="modal-header">' +
      '  <h3 id="' + titleId + '">' + (global.AuroraDom ? global.AuroraDom.esc(title) : title) + '</h3>' +
      '  <button class="modal-close" aria-label="Close">×</button>' +
      '</div>' +
      '<div class="modal-body"></div>' +
      (footerHtml || spec.footer instanceof Node ? '<div class="modal-footer"></div>' : '');

    const body = modal.querySelector('.modal-body');
    if (spec.body instanceof Node) body.appendChild(spec.body);
    else body.innerHTML = bodyHtml;

    const footer = modal.querySelector('.modal-footer');
    if (footer) {
      if (spec.footer instanceof Node) footer.appendChild(spec.footer);
      else if (footerHtml) footer.innerHTML = footerHtml;
    }

    const overlay = ensureOverlay();
    overlay.classList.add('active');
    root.appendChild(modal);

    const dismissible = spec.dismissible !== false;
    const closeBtn = modal.querySelector('.modal-close');
    if (closeBtn) closeBtn.addEventListener('click', close);
    if (dismissible) {
      overlay.addEventListener('click', overlayClick);
    } else {
      overlay.removeEventListener('click', overlayClick);
    }
    document.addEventListener('keydown', escClose);

    if (global.AuroraA11y) {
      releaseTrap = global.AuroraA11y.trapFocus(modal);
    }

    activeHandle = {
      modal: modal,
      close: close,
      onClose: spec.onClose || null,
    };
    return activeHandle;
  }

  function close() {
    if (!activeHandle) return;
    const { modal, onClose } = activeHandle;
    if (releaseTrap) { try { releaseTrap(); } catch (e) {} releaseTrap = null; }
    document.removeEventListener('keydown', escClose);
    const overlay = document.getElementById('modal-overlay');
    if (overlay) {
      overlay.classList.remove('active');
      overlay.removeEventListener('click', overlayClick);
    }
    if (modal && modal.parentNode) modal.parentNode.removeChild(modal);
    activeHandle = null;
    if (typeof onClose === 'function') {
      try { onClose(); } catch (e) { /* ignore */ }
    }
  }

  function overlayClick(e) {
    // Only close on overlay click, not modal interior.
    if (e.target.id === 'modal-overlay') close();
  }

  function escClose(e) {
    if (e.key === 'Escape') close();
  }

  function ensureRoot() {
    let root = document.getElementById('modal-root');
    if (!root) {
      root = document.createElement('div');
      root.id = 'modal-root';
      document.body.appendChild(root);
    }
    return root;
  }

  function ensureOverlay() {
    let overlay = document.getElementById('modal-overlay');
    if (!overlay) {
      overlay = document.createElement('div');
      overlay.id = 'modal-overlay';
      overlay.className = 'modal-overlay';
      document.body.appendChild(overlay);
    }
    return overlay;
  }

  global.AuroraModal = {
    open: open,
    close: close,
  };
})(window);
