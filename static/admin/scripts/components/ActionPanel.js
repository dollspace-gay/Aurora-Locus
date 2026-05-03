// ActionPanel substrate primitive (substrate primitive 3).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §6.3:
//   The single component for action affordances across the UI.
//   One component implements pick action → configure → rationale →
//   confirm. Pages compose it into their action surfaces.
//
// Vanilla JS, no build steps (per §12.9). Constructor returns an
// instance with `mount(container)` / `unmount()` / `getValue()`
// helpers. `submit()` dispatches via AuroraCapabilities.callEndpoint
// when the unified emit-mod-event path is available, else falls back
// to a per-action endpoint chosen by the caller.
//
// Accessibility contract per §6.3:
//   - Action dropdown: role=combobox with proper labeling
//   - Rationale textarea: aria-required="true", aria-describedby for hints
//   - Typed-confirmation: aria-required="true", validation via aria-live="polite"
//   - High-impact checkbox: standard checkbox with <label> association
//   - Confirm: aria-disabled until valid; reason announced when focused
//   - Form: Enter in textarea inserts newline; Tab+Enter submits

(function (global) {
  'use strict';

  // ---------- Action catalog ----------
  //
  // ACTION_DEFS describes each action's display name, ModEventAction
  // discriminator (kind + inline data shape), default rationale
  // requirement, and high-impact flag. Pages pass `availableActions`
  // (a subset of these names) per their context.
  //
  // The discriminator shape lines up with src/api/aurora_admin.rs's
  // ModEventAction enum; the runtime payload built by submit()
  // matches what the server expects.
  const ACTION_DEFS = {
    TakedownAccount: {
      label: 'Takedown account',
      kind: 'TakedownAccount',
      destructive: true,
      highImpact: true,
      validSubjectTypes: ['repo'],
    },
    SuspendAccount: {
      label: 'Suspend account',
      kind: 'SuspendAccount',
      destructive: true,
      highImpact: true,
      validSubjectTypes: ['repo'],
      // Optional metadata: durationDays
      metadataFields: [
        { key: 'durationDays', label: 'Duration (days)', type: 'number', optional: true },
      ],
    },
    RestoreAccount: {
      label: 'Restore account',
      kind: 'RestoreAccount',
      destructive: false,
      highImpact: false,
      validSubjectTypes: ['repo'],
    },
    DeleteAccount: {
      label: 'Delete account permanently',
      kind: 'DeleteAccount',
      destructive: true,
      highImpact: true,
      requiresAdminRole: true,
      validSubjectTypes: ['repo'],
    },
    ApplyLabel: {
      label: 'Apply label',
      kind: 'ApplyLabel',
      destructive: false,
      highImpact: false,
      validSubjectTypes: ['repo', 'record', 'blob'],
      inlineFields: [
        { key: 'val', label: 'Label value', type: 'text', required: true },
      ],
    },
    RemoveLabel: {
      label: 'Remove label',
      kind: 'RemoveLabel',
      destructive: false,
      highImpact: false,
      validSubjectTypes: ['repo', 'record', 'blob'],
      inlineFields: [
        { key: 'val', label: 'Label value', type: 'text', required: true },
      ],
    },
    TakedownRecord: {
      label: 'Takedown record',
      kind: 'TakedownRecord',
      destructive: true,
      highImpact: true,
      validSubjectTypes: ['record'],
    },
    QuarantineBlob: {
      label: 'Quarantine blob',
      kind: 'QuarantineBlob',
      destructive: true,
      highImpact: true,
      validSubjectTypes: ['blob'],
    },
    RestoreBlob: {
      label: 'Restore blob',
      kind: 'RestoreBlob',
      destructive: false,
      highImpact: false,
      validSubjectTypes: ['blob'],
    },
    ResolveAppeal: {
      label: 'Resolve appeal',
      kind: 'ResolveAppeal',
      destructive: false,
      highImpact: true,
      // Subject type unconstrained — appeal carries its own subject ref.
      validSubjectTypes: ['repo', 'record', 'blob'],
      inlineFields: [
        { key: 'appealId', label: 'Appeal ID', type: 'number', required: true },
        {
          key: 'resolution',
          label: 'Decision',
          type: 'select',
          options: [
            { value: 'approve', label: 'Approve (cascade reversal)' },
            { value: 'deny', label: 'Deny' },
          ],
          required: true,
        },
      ],
    },
    EscalateAppeal: {
      label: 'Escalate appeal',
      kind: 'EscalateAppeal',
      destructive: false,
      highImpact: false,
      validSubjectTypes: ['repo', 'record', 'blob'],
      inlineFields: [
        { key: 'appealId', label: 'Appeal ID', type: 'number', required: true },
      ],
    },
    SendEmail: {
      label: 'Send email',
      kind: 'SendEmail',
      destructive: false,
      highImpact: false,
      validSubjectTypes: ['repo'],
      inlineFields: [
        { key: 'subject', label: 'Email subject', type: 'text', required: true },
        { key: 'body', label: 'Email body', type: 'textarea', required: true },
      ],
    },
  };

  // ---------- Subject formatting helpers ----------

  function subjectType(subject) {
    if (!subject || !subject.$type) return null;
    const t = subject.$type;
    if (t.endsWith('repoRef')) return 'repo';
    if (t.endsWith('strongRef')) return 'record';
    if (t.endsWith('repoBlobRef')) return 'blob';
    return null;
  }

  function formatSubject(subject) {
    const t = subjectType(subject);
    if (t === 'repo') return subject.did;
    if (t === 'record') return subject.uri;
    if (t === 'blob') return 'blob ' + subject.cid + ' on ' + (subject.did || '');
    return JSON.stringify(subject);
  }

  // ---------- Rendering ----------

  function el(tag, props, children) {
    const node = document.createElement(tag);
    if (props) {
      for (const k of Object.keys(props)) {
        if (k === 'class') node.className = props[k];
        else if (k === 'style') node.setAttribute('style', props[k]);
        else if (k.startsWith('on') && typeof props[k] === 'function') {
          node.addEventListener(k.substring(2).toLowerCase(), props[k]);
        } else if (k === 'textContent') node.textContent = props[k];
        else node.setAttribute(k, props[k]);
      }
    }
    if (children) {
      for (const c of children) {
        if (c == null) continue;
        if (typeof c === 'string') node.appendChild(document.createTextNode(c));
        else node.appendChild(c);
      }
    }
    return node;
  }

  // ---------- ActionPanel class ----------

  function ActionPanel(opts) {
    this.subject = opts.subject;
    this.availableActions = opts.availableActions || [];
    this.defaultAction = opts.defaultAction || null;
    this.requiresRationale = opts.requiresRationale !== false; // default true
    this.defaultRationale = opts.defaultRationale || '';
    this.highImpactActions = new Set(opts.highImpactActions || []);
    this.onConfirm = opts.onConfirm;
    this.onCancel = opts.onCancel || function () {};
    this.userRole = opts.userRole || 'moderator';
    this.container = null;
    this.dom = null;
    this.state = {
      action: this.defaultAction || (this.availableActions[0] || null),
      rationale: this.defaultRationale,
      inlineData: {},
      metadata: {},
      typedConfirm: '',
      ackChecked: false,
      submitting: false,
      error: null,
    };
  }

  ActionPanel.prototype.mount = function (container) {
    this.container = container;
    this.render();
  };

  ActionPanel.prototype.unmount = function () {
    if (this.dom && this.dom.parentNode) {
      this.dom.parentNode.removeChild(this.dom);
    }
    this.dom = null;
    this.container = null;
  };

  ActionPanel.prototype.render = function () {
    if (!this.container) return;
    this.container.innerHTML = '';
    const def = this.state.action ? ACTION_DEFS[this.state.action] : null;
    const isHighImpact = this.state.action && this.highImpactActions.has(this.state.action);
    const subjectStr = formatSubject(this.subject);

    // Action select — role=combobox per §6.3 a11y contract
    const actionSelectId = 'action-panel-action-' + Math.random().toString(36).slice(2);
    const actionSelect = el('select', {
      id: actionSelectId,
      'aria-label': 'Action',
      class: 'action-panel-action',
    });
    for (const name of this.availableActions) {
      const d = ACTION_DEFS[name];
      if (!d) continue;
      const opt = el('option', { value: name }, [d.label]);
      if (this.state.action === name) opt.setAttribute('selected', 'selected');
      actionSelect.appendChild(opt);
    }
    actionSelect.addEventListener('change', (e) => {
      this.state.action = e.target.value;
      this.state.inlineData = {};
      this.state.metadata = {};
      this.state.error = null;
      this.render();
    });

    // Subject display
    const subjectBlock = el('div', { class: 'action-panel-subject' }, [
      el('div', { class: 'action-panel-label' }, ['Subject']),
      el('div', { class: 'action-panel-subject-value' }, [subjectStr]),
    ]);

    // Inline fields per action
    const inlineFields = [];
    if (def && def.inlineFields) {
      for (const f of def.inlineFields) {
        inlineFields.push(this.renderField(f, this.state.inlineData, 'inlineData'));
      }
    }
    if (def && def.metadataFields) {
      for (const f of def.metadataFields) {
        inlineFields.push(this.renderField(f, this.state.metadata, 'metadata'));
      }
    }

    // Rationale textarea
    const rationaleId = 'action-panel-rationale-' + Math.random().toString(36).slice(2);
    const rationaleHelpId = rationaleId + '-help';
    const rationaleProps = {
      id: rationaleId,
      class: 'action-panel-rationale',
      'aria-describedby': rationaleHelpId,
      rows: '4',
    };
    if (this.requiresRationale) rationaleProps['aria-required'] = 'true';
    const rationaleEl = el('textarea', rationaleProps);
    rationaleEl.value = this.state.rationale;
    rationaleEl.addEventListener('input', (e) => {
      this.state.rationale = e.target.value;
      this.refreshConfirmButton();
    });

    // High-impact gates
    let typedConfirmEl = null;
    let ackCheckboxEl = null;
    let typedConfirmFieldId = null;
    let ackCheckboxId = null;
    if (def && (def.highImpact || isHighImpact)) {
      typedConfirmFieldId =
        'action-panel-typed-' + Math.random().toString(36).slice(2);
      typedConfirmEl = el('input', {
        type: 'text',
        id: typedConfirmFieldId,
        'aria-required': 'true',
        'aria-label': 'Type CONFIRM to enable',
        placeholder: 'Type CONFIRM',
      });
      typedConfirmEl.addEventListener('input', (e) => {
        this.state.typedConfirm = e.target.value;
        this.refreshConfirmButton();
      });
      ackCheckboxId = 'action-panel-ack-' + Math.random().toString(36).slice(2);
      ackCheckboxEl = el('input', { type: 'checkbox', id: ackCheckboxId });
      ackCheckboxEl.addEventListener('change', (e) => {
        this.state.ackChecked = e.target.checked;
        this.refreshConfirmButton();
      });
    }

    // Buttons
    const confirmText = def && def.destructive ? 'Confirm' : 'Confirm';
    const confirmBtn = el(
      'button',
      {
        type: 'button',
        class: def && def.destructive ? 'btn-danger' : 'btn-primary',
      },
      [confirmText],
    );
    confirmBtn.addEventListener('click', (e) => {
      e.preventDefault();
      this.submit();
    });
    this._confirmBtn = confirmBtn;
    const cancelBtn = el('button', { type: 'button', class: 'btn-secondary' }, ['Cancel']);
    cancelBtn.addEventListener('click', () => this.onCancel());

    // Status / error region — aria-live="polite" per a11y contract
    const statusEl = el('div', {
      class: 'action-panel-status',
      'aria-live': 'polite',
      role: 'status',
    });
    if (this.state.error) {
      statusEl.textContent = 'Error: ' + this.state.error;
      statusEl.classList.add('action-panel-status-error');
    }
    this._statusEl = statusEl;

    const formChildren = [
      el('h3', { class: 'action-panel-title' }, ['Take action']),
      el('div', { class: 'action-panel-row' }, [
        el('label', { for: actionSelectId, class: 'action-panel-label' }, ['Action']),
        actionSelect,
      ]),
      subjectBlock,
    ];
    inlineFields.forEach((f) => formChildren.push(f));
    formChildren.push(
      el('div', { class: 'action-panel-row' }, [
        el(
          'label',
          { for: rationaleId, class: 'action-panel-label' },
          [this.requiresRationale ? 'Rationale (required)' : 'Rationale (optional)'],
        ),
        rationaleEl,
        el(
          'div',
          { id: rationaleHelpId, class: 'action-panel-hint' },
          [
            this.requiresRationale
              ? 'Rationale is required and recorded in the audit log.'
              : 'Rationale is optional but recommended.',
          ],
        ),
      ]),
    );
    if (typedConfirmEl) {
      formChildren.push(
        el('div', { class: 'action-panel-row action-panel-high-impact' }, [
          el(
            'label',
            { for: typedConfirmFieldId, class: 'action-panel-label' },
            ['Type CONFIRM to enable'],
          ),
          typedConfirmEl,
          el('div', { class: 'action-panel-hint' }, [
            'High-impact action. Type CONFIRM exactly to unlock the button.',
          ]),
        ]),
      );
    }
    if (ackCheckboxEl) {
      formChildren.push(
        el('div', { class: 'action-panel-row action-panel-ack' }, [
          ackCheckboxEl,
          el('label', { for: ackCheckboxId, class: 'action-panel-label-inline' }, [
            'I understand this affects all federation.',
          ]),
        ]),
      );
    }
    formChildren.push(statusEl);
    formChildren.push(
      el('div', { class: 'action-panel-buttons' }, [cancelBtn, confirmBtn]),
    );

    this.dom = el('div', { class: 'action-panel-card', role: 'group', 'aria-label': 'Action panel' }, formChildren);
    this.container.appendChild(this.dom);
    this.refreshConfirmButton();
  };

  ActionPanel.prototype.renderField = function (field, target, _bucket) {
    const fieldId = 'action-panel-field-' + field.key + '-' + Math.random().toString(36).slice(2);
    let inputEl;
    if (field.type === 'select') {
      inputEl = el('select', { id: fieldId });
      for (const opt of field.options) {
        const o = el('option', { value: opt.value }, [opt.label]);
        if (target[field.key] === opt.value) o.setAttribute('selected', 'selected');
        inputEl.appendChild(o);
      }
      // Pre-select the first option's value into state if empty
      if (target[field.key] == null && field.options.length > 0) {
        target[field.key] = field.options[0].value;
      }
      inputEl.addEventListener('change', (e) => {
        target[field.key] = e.target.value;
        this.refreshConfirmButton();
      });
    } else if (field.type === 'textarea') {
      inputEl = el('textarea', { id: fieldId, rows: '3' });
      inputEl.value = target[field.key] || '';
      inputEl.addEventListener('input', (e) => {
        target[field.key] = e.target.value;
        this.refreshConfirmButton();
      });
    } else {
      const t = field.type === 'number' ? 'number' : 'text';
      inputEl = el('input', { type: t, id: fieldId });
      if (target[field.key] != null) inputEl.value = target[field.key];
      inputEl.addEventListener('input', (e) => {
        target[field.key] = field.type === 'number' ? Number(e.target.value) : e.target.value;
        this.refreshConfirmButton();
      });
    }
    if (field.required) inputEl.setAttribute('aria-required', 'true');
    return el('div', { class: 'action-panel-row' }, [
      el('label', { for: fieldId, class: 'action-panel-label' }, [
        field.label + (field.required ? ' (required)' : ' (optional)'),
      ]),
      inputEl,
    ]);
  };

  ActionPanel.prototype.refreshConfirmButton = function () {
    if (!this._confirmBtn) return;
    const reason = this.validityReason();
    if (reason) {
      this._confirmBtn.setAttribute('disabled', 'disabled');
      this._confirmBtn.setAttribute('aria-disabled', 'true');
      this._confirmBtn.setAttribute('title', reason);
      this._confirmBtn.setAttribute('aria-describedby', '');
    } else {
      this._confirmBtn.removeAttribute('disabled');
      this._confirmBtn.removeAttribute('aria-disabled');
      this._confirmBtn.removeAttribute('title');
    }
  };

  ActionPanel.prototype.validityReason = function () {
    if (this.state.submitting) return 'Submitting…';
    if (!this.state.action) return 'Select an action';
    const def = ACTION_DEFS[this.state.action];
    if (!def) return 'Unknown action';
    // Subject-type compat
    const stype = subjectType(this.subject);
    if (stype && def.validSubjectTypes.indexOf(stype) === -1) {
      return def.label + ' does not apply to ' + stype + ' subjects';
    }
    // Rationale required
    if (this.requiresRationale && !(this.state.rationale && this.state.rationale.trim())) {
      return 'Rationale is required';
    }
    // Inline fields required
    if (def.inlineFields) {
      for (const f of def.inlineFields) {
        if (f.required) {
          const v = this.state.inlineData[f.key];
          if (v == null || v === '' || (typeof v === 'string' && v.trim() === '')) {
            return f.label + ' is required';
          }
        }
      }
    }
    // High-impact gates
    const isHigh = def.highImpact || this.highImpactActions.has(this.state.action);
    if (isHigh) {
      if (this.state.typedConfirm !== 'CONFIRM') {
        return 'Type CONFIRM to enable';
      }
      if (!this.state.ackChecked) {
        return 'Acknowledge the federation impact';
      }
    }
    // Admin-role check (display-side; server is authoritative per §3.1)
    if (def.requiresAdminRole && this.userRole !== 'admin' && this.userRole !== 'superadmin') {
      return 'This action requires an Admin role';
    }
    return null;
  };

  // Build the EmitEventInput payload from current state.
  ActionPanel.prototype.buildPayload = function () {
    const def = ACTION_DEFS[this.state.action];
    if (!def) throw new Error('No action selected');
    const action = { kind: def.kind };
    if (def.inlineFields) {
      for (const f of def.inlineFields) {
        const v = this.state.inlineData[f.key];
        if (v != null && v !== '') action[f.key] = v;
      }
    }
    const payload = {
      action: action,
      subject: this.subject,
      rationale: this.state.rationale,
      snapshotCapture: true,
    };
    if (Object.keys(this.state.metadata).length > 0) {
      payload.metadata = {};
      for (const k of Object.keys(this.state.metadata)) {
        if (this.state.metadata[k] != null && this.state.metadata[k] !== '') {
          payload.metadata[k] = this.state.metadata[k];
        }
      }
    }
    return payload;
  };

  ActionPanel.prototype.submit = async function () {
    const reason = this.validityReason();
    if (reason) {
      this.state.error = reason;
      this.render();
      return;
    }
    this.state.submitting = true;
    this.state.error = null;
    this.refreshConfirmButton();
    try {
      const payload = this.buildPayload();
      let result;
      if (this.onConfirm) {
        // Caller supplied a custom dispatcher (used by callers that
        // can't or shouldn't use the unified emitEvent — e.g. tests).
        result = await this.onConfirm(this.state.action, this.state.rationale, payload);
      } else {
        // Default: route through capability substrate to emitEvent
        // when available. Pre-3.5 fallback is the page's per-action
        // endpoint, which the page can implement by passing onConfirm.
        result = await global.AuroraCapabilities.callEndpoint('emit-mod-event', payload);
      }
      this.state.submitting = false;
      if (this._statusEl) {
        this._statusEl.textContent = 'Action submitted successfully';
        this._statusEl.classList.remove('action-panel-status-error');
      }
      this.refreshConfirmButton();
      return result;
    } catch (e) {
      this.state.submitting = false;
      this.state.error = e && e.message ? e.message : String(e);
      this.render();
      throw e;
    }
  };

  global.ActionPanel = ActionPanel;
  global.ActionPanel.ACTION_DEFS = ACTION_DEFS;
  global.ActionPanel.subjectType = subjectType;
  global.ActionPanel.formatSubject = formatSubject;
})(window);
