// Modal substrate primitive.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.6 + §7.10 and
// docs/V04_DESIGN.md §5.3.3. A single modal surface mounts into
// #modal-root with focus trap, Esc-to-close, and overlay click
// handling. Pages don't manipulate DOM directly — they open a modal
// via openModal(spec) and close it via the returned handle or
// AuroraModal.close().
//
// Core API (Arc 6 Step 1 era):
//   const handle = AuroraModal.open({
//     title: 'Action',
//     body: htmlString | Node,
//     footer?: htmlString | Node,
//     dismissible?: bool (default true),
//     onClose?: () => void,
//   });
//   handle.close();
//   AuroraModal.close();   // close current
//
// Helper API (Arc 6 Step 4 — composes open()):
//   AuroraModal.form({ heading, body, fields, submitLabel })
//     → Promise<{ submitted: bool, values?: object }>
//
//     fields: [{ name, label, type, required?, placeholder?,
//       default?, validate?(value) → string|null }]
//     type ∈ 'text' | 'password' | 'textarea' | 'checkbox'
//
//     fields: [] is the non-destructive yes/no shape — body
//     text is the question; submitting confirms.
//
//   AuroraModal.destructiveConfirm({ heading, body,
//     typedConfirmGate, rationaleRequired, ackCheckbox,
//     confirmLabel })
//     → Promise<{ confirmed: bool, rationale?: string,
//       ackChecked?: bool }>
//
//     typedConfirmGate: string|null. When non-null, submit
//     stays disabled until operator types the exact string
//     (case-sensitive) into the gate input.
//     rationaleRequired: bool. When true, the resolved
//     promise's values include the operator's rationale.
//     ackCheckbox: string|null. When non-null, the named
//     checkbox must be checked to enable submit.

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

  // ---------- Helper API ----------

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  function uniqueId(prefix) {
    return prefix + '-' + Math.random().toString(36).slice(2, 10);
  }

  // form(spec) — collects 1+ fields (or zero for yes/no confirm)
  // and resolves a promise on submit/cancel. Composes open() under
  // the hood; the body Node and footer Node are built here.
  function form(spec) {
    spec = spec || {};
    const fields = Array.isArray(spec.fields) ? spec.fields : [];
    const submitLabel = spec.submitLabel || 'Confirm';

    return new Promise((resolve) => {
      const body = document.createElement('div');
      let html = '';
      if (spec.body) html += '<p class="modal-body-text">' + esc(spec.body) + '</p>';
      const fieldIds = {};
      for (const f of fields) {
        const fid = uniqueId('mf-' + (f.name || 'field'));
        fieldIds[f.name] = fid;
        const required = f.required !== false ? ' aria-required="true"' : '';
        const ph = f.placeholder ? ' placeholder="' + esc(f.placeholder) + '"' : '';
        const defValHtml = f.default != null ? esc(f.default) : '';
        html += '<div class="form-group">' +
                '<label for="' + fid + '">' + esc(f.label || f.name) +
                (f.required !== false ? ' <span class="required-marker" aria-hidden="true">*</span>' : '') +
                '</label>';
        if (f.type === 'textarea') {
          html += '<textarea id="' + fid + '" rows="3"' + required + ph + '>' + defValHtml + '</textarea>';
        } else if (f.type === 'checkbox') {
          html += '<label class="checkbox-label">' +
                  '<input type="checkbox" id="' + fid + '"' +
                  (f.default ? ' checked' : '') + '> ' + esc(f.label || '') +
                  '</label>';
        } else {
          const t = f.type === 'password' ? 'password' : 'text';
          html += '<input type="' + t + '" id="' + fid + '"' + required + ph +
                  ' value="' + defValHtml + '">';
        }
        html += '<div class="form-error" id="' + fid + '-err" aria-live="polite"></div>';
        html += '</div>';
      }
      body.innerHTML = html;

      const footer = document.createElement('div');
      footer.className = 'modal-buttons';
      const cancelId = uniqueId('mf-cancel');
      const submitId = uniqueId('mf-submit');
      footer.innerHTML =
        '<button type="button" class="btn-secondary" id="' + cancelId + '">Cancel</button>' +
        '<button type="button" class="btn-primary" id="' + submitId + '">' + esc(submitLabel) + '</button>';

      let resolved = false;
      const handle = open({
        title: spec.heading || '',
        body: body,
        footer: footer,
        onClose: () => {
          if (!resolved) {
            resolved = true;
            resolve({ submitted: false });
          }
        },
      });

      const submitBtn = footer.querySelector('#' + submitId);
      const cancelBtn = footer.querySelector('#' + cancelId);

      function readValues() {
        const values = {};
        for (const f of fields) {
          const el = body.querySelector('#' + fieldIds[f.name]);
          if (!el) continue;
          values[f.name] = f.type === 'checkbox' ? el.checked : el.value;
        }
        return values;
      }

      function validate() {
        const values = readValues();
        let valid = true;
        for (const f of fields) {
          const errEl = body.querySelector('#' + fieldIds[f.name] + '-err');
          if (!errEl) continue;
          errEl.textContent = '';
          const v = values[f.name];
          const required = f.required !== false;
          if (required) {
            if (f.type === 'checkbox') {
              if (!v) valid = false;
            } else if (v == null || (typeof v === 'string' && v.trim() === '')) {
              valid = false;
            }
          }
          if (valid && typeof f.validate === 'function') {
            const msg = f.validate(v);
            if (msg) {
              valid = false;
              errEl.textContent = msg;
            }
          }
        }
        return valid;
      }

      function refreshSubmit() {
        const valid = validate();
        if (valid) {
          submitBtn.removeAttribute('disabled');
          submitBtn.removeAttribute('aria-disabled');
        } else {
          submitBtn.setAttribute('disabled', 'disabled');
          submitBtn.setAttribute('aria-disabled', 'true');
        }
      }

      // Wire inputs to live-refresh the submit-enabled state.
      for (const f of fields) {
        const el = body.querySelector('#' + fieldIds[f.name]);
        if (!el) continue;
        const evt = f.type === 'checkbox' ? 'change' : 'input';
        el.addEventListener(evt, refreshSubmit);
      }

      submitBtn.addEventListener('click', () => {
        if (!validate()) return;
        const values = readValues();
        resolved = true;
        handle.close();
        resolve({ submitted: true, values: values });
      });
      cancelBtn.addEventListener('click', () => {
        resolved = true;
        handle.close();
        resolve({ submitted: false });
      });

      // Focus: first field if any, else submit button.
      setTimeout(() => {
        if (fields.length > 0) {
          const firstEl = body.querySelector('#' + fieldIds[fields[0].name]);
          if (firstEl && typeof firstEl.focus === 'function') firstEl.focus();
        } else {
          submitBtn.focus();
        }
      }, 0);

      refreshSubmit();
    });
  }

  // destructiveConfirm(spec) — typed-gate + optional rationale +
  // optional ack checkbox. Resolves with confirmation outcome and
  // the rationale text when collected.
  function destructiveConfirm(spec) {
    spec = spec || {};
    const typedGate = (typeof spec.typedConfirmGate === 'string' && spec.typedConfirmGate.length > 0)
      ? spec.typedConfirmGate
      : null;
    const rationaleRequired = !!spec.rationaleRequired;
    const ackLabel = (typeof spec.ackCheckbox === 'string' && spec.ackCheckbox.length > 0)
      ? spec.ackCheckbox
      : null;
    const confirmLabel = spec.confirmLabel || 'Confirm';

    return new Promise((resolve) => {
      const body = document.createElement('div');
      const gateId = uniqueId('dc-gate');
      const rationaleId = uniqueId('dc-rationale');
      const ackId = uniqueId('dc-ack');

      let html = '';
      if (spec.body) {
        html += '<p class="modal-body-text">' + esc(spec.body) + '</p>';
      }
      if (typedGate != null) {
        html += '<div class="form-group">' +
                '<label for="' + gateId + '">' +
                'Type <code>' + esc(typedGate) + '</code> to confirm' +
                ' <span class="required-marker" aria-hidden="true">*</span>' +
                '</label>' +
                '<input type="text" id="' + gateId + '" autocomplete="off" aria-required="true">' +
                '</div>';
      }
      if (rationaleRequired) {
        html += '<div class="form-group">' +
                '<label for="' + rationaleId + '">' +
                'Rationale (recorded in audit log)' +
                ' <span class="required-marker" aria-hidden="true">*</span>' +
                '</label>' +
                '<textarea id="' + rationaleId + '" rows="3" aria-required="true"></textarea>' +
                '</div>';
      }
      if (ackLabel != null) {
        html += '<div class="form-group action-panel-ack">' +
                '<label class="checkbox-label">' +
                '<input type="checkbox" id="' + ackId + '" aria-required="true"> ' +
                esc(ackLabel) +
                '</label>' +
                '</div>';
      }
      body.innerHTML = html;

      const footer = document.createElement('div');
      footer.className = 'modal-buttons';
      const cancelId = uniqueId('dc-cancel');
      const submitId = uniqueId('dc-submit');
      footer.innerHTML =
        '<button type="button" class="btn-secondary" id="' + cancelId + '">Cancel</button>' +
        '<button type="button" class="btn-danger" id="' + submitId + '">' + esc(confirmLabel) + '</button>';

      let resolved = false;
      const handle = open({
        title: spec.heading || '',
        body: body,
        footer: footer,
        onClose: () => {
          if (!resolved) {
            resolved = true;
            resolve({ confirmed: false });
          }
        },
      });

      const gateEl = typedGate != null ? body.querySelector('#' + gateId) : null;
      const rationaleEl = rationaleRequired ? body.querySelector('#' + rationaleId) : null;
      const ackEl = ackLabel != null ? body.querySelector('#' + ackId) : null;
      const submitBtn = footer.querySelector('#' + submitId);
      const cancelBtn = footer.querySelector('#' + cancelId);

      function isValid() {
        if (gateEl && gateEl.value !== typedGate) return false;
        if (rationaleEl && rationaleEl.value.trim() === '') return false;
        if (ackEl && !ackEl.checked) return false;
        return true;
      }

      function refreshSubmit() {
        if (isValid()) {
          submitBtn.removeAttribute('disabled');
          submitBtn.removeAttribute('aria-disabled');
        } else {
          submitBtn.setAttribute('disabled', 'disabled');
          submitBtn.setAttribute('aria-disabled', 'true');
        }
      }

      if (gateEl) gateEl.addEventListener('input', refreshSubmit);
      if (rationaleEl) rationaleEl.addEventListener('input', refreshSubmit);
      if (ackEl) ackEl.addEventListener('change', refreshSubmit);

      submitBtn.addEventListener('click', () => {
        if (!isValid()) return;
        const result = { confirmed: true };
        if (rationaleEl) result.rationale = rationaleEl.value;
        if (ackEl) result.ackChecked = ackEl.checked;
        resolved = true;
        handle.close();
        resolve(result);
      });
      cancelBtn.addEventListener('click', () => {
        resolved = true;
        handle.close();
        resolve({ confirmed: false });
      });

      // Focus per spec: gate input first, else rationale, else
      // cancel (defensive — operator must move forward deliberately
      // when no gate/rationale demands attention).
      setTimeout(() => {
        const target = gateEl || rationaleEl || cancelBtn;
        if (target && typeof target.focus === 'function') target.focus();
      }, 0);

      refreshSubmit();
    });
  }

  global.AuroraModal = {
    open: open,
    close: close,
    form: form,
    destructiveConfirm: destructiveConfirm,
  };
})(window);
