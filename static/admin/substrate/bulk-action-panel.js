// BulkActionPanel substrate primitive (substrate primitive 4).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §6.4:
//   Specialized variant of ActionPanel for batch operations on
//   multiple subjects. Same shape as ActionPanel but with batched
//   semantics: one rationale, one audit entry, one confirmation.
//
// Vanilla JS, no build steps (per §12.9). Routes through
// AuroraCapabilities.callEndpoint to the matching batch endpoint.
// 50-subject hard cap (§6.4 visual contract).
//
// Accessibility contract per §6.4:
//   - Same as ActionPanel
//   - Subject count announces with action label: "Confirm bulk
//     takedown of 3 accounts"
//   - Subjects list aria-label="Affected subjects"

(function (global) {
  'use strict';

  const MAX_BATCH_SIZE = 50; // §6.4 visual contract

  // Action → (feature, payload-builder). Only actions backed by a
  // batch endpoint show up here; mapping mirrors §8.8–§8.13.
  const BATCH_ACTIONS = {
    BatchTakedownAccounts: {
      label: 'Takedown selected accounts',
      feature: 'batch-takedown-accounts',
      destructive: true,
      validSubjectTypes: ['repo'],
      buildPayload: function (subjects, rationale) {
        return {
          dids: subjects.map((s) => s.did),
          rationale: rationale,
        };
      },
    },
    BatchSuspendAccounts: {
      label: 'Suspend selected accounts',
      feature: 'batch-suspend-accounts',
      destructive: true,
      validSubjectTypes: ['repo'],
      buildPayload: function (subjects, rationale) {
        return { dids: subjects.map((s) => s.did), rationale: rationale };
      },
    },
    BatchRestoreAccounts: {
      label: 'Restore selected accounts',
      feature: 'batch-restore-accounts',
      destructive: false,
      validSubjectTypes: ['repo'],
      buildPayload: function (subjects, rationale) {
        return { dids: subjects.map((s) => s.did), rationale: rationale };
      },
    },
    BatchTakedownRecords: {
      label: 'Takedown selected records',
      feature: 'batch-takedown-records',
      destructive: true,
      validSubjectTypes: ['record'],
      buildPayload: function (subjects, rationale) {
        return { uris: subjects.map((s) => s.uri), rationale: rationale };
      },
    },
    BatchApplyLabel: {
      label: 'Apply label to selected',
      feature: 'batch-apply-label',
      destructive: false,
      validSubjectTypes: ['repo', 'record', 'blob'],
      inlineFields: [
        { key: 'label_val', label: 'Label value', type: 'text', required: true },
      ],
      buildPayload: function (subjects, rationale, inlineData) {
        return {
          subjects: subjects,
          labelVal: inlineData.label_val,
          labelNeg: false,
          rationale: rationale,
        };
      },
    },
    BatchRemoveLabel: {
      label: 'Remove label from selected',
      feature: 'batch-remove-label',
      destructive: false,
      validSubjectTypes: ['repo', 'record', 'blob'],
      inlineFields: [
        { key: 'label_val', label: 'Label value', type: 'text', required: true },
      ],
      buildPayload: function (subjects, rationale, inlineData) {
        return {
          subjects: subjects,
          labelVal: inlineData.label_val,
          labelNeg: false,
          rationale: rationale,
        };
      },
    },
  };

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

  function subjectType(subject) {
    if (!subject || !subject.$type) return null;
    if (subject.$type.endsWith('repoRef')) return 'repo';
    if (subject.$type.endsWith('strongRef')) return 'record';
    if (subject.$type.endsWith('repoBlobRef')) return 'blob';
    return null;
  }

  function formatSubject(s) {
    const t = subjectType(s);
    if (t === 'repo') return s.did;
    if (t === 'record') return s.uri;
    if (t === 'blob') return 'blob ' + s.cid;
    return JSON.stringify(s);
  }

  function BulkActionPanel(opts) {
    this.subjects = opts.subjects || [];
    this.availableActions = opts.availableActions || [];
    this.onConfirm = opts.onConfirm;
    this.onCancel = opts.onCancel || function () {};
    this.maxBatchSize = opts.maxBatchSize || MAX_BATCH_SIZE;
    this.container = null;
    this.dom = null;
    this.state = {
      action: this.availableActions[0] || null,
      rationale: '',
      inlineData: {},
      submitting: false,
      error: null,
      result: null,
    };
  }

  BulkActionPanel.prototype.mount = function (container) {
    this.container = container;
    this.render();
  };

  BulkActionPanel.prototype.unmount = function () {
    if (this.dom && this.dom.parentNode) {
      this.dom.parentNode.removeChild(this.dom);
    }
    this.dom = null;
    this.container = null;
  };

  BulkActionPanel.prototype.render = function () {
    if (!this.container) return;
    this.container.innerHTML = '';
    const def = this.state.action ? BATCH_ACTIONS[this.state.action] : null;
    const subjectCount = this.subjects.length;

    // Action select
    const actionSelectId = 'bulk-action-' + Math.random().toString(36).slice(2);
    const actionSelect = el('select', { id: actionSelectId, 'aria-label': 'Bulk action' });
    for (const name of this.availableActions) {
      const d = BATCH_ACTIONS[name];
      if (!d) continue;
      const opt = el('option', { value: name }, [d.label]);
      if (this.state.action === name) opt.setAttribute('selected', 'selected');
      actionSelect.appendChild(opt);
    }
    actionSelect.addEventListener('change', (e) => {
      this.state.action = e.target.value;
      this.state.inlineData = {};
      this.state.error = null;
      this.render();
    });

    // Subjects list (collapsible if longer than 10 per §6.4)
    const subjectsListId = 'bulk-subjects-' + Math.random().toString(36).slice(2);
    const subjectItems = this.subjects.slice(0, 10).map((s) =>
      el('li', null, [formatSubject(s)]),
    );
    const overflow = this.subjects.length - 10;
    const subjectList = el(
      'ul',
      { id: subjectsListId, 'aria-label': 'Affected subjects', class: 'bulk-subjects-list' },
      subjectItems,
    );
    if (overflow > 0) {
      subjectList.appendChild(el('li', { class: 'bulk-subjects-overflow' }, ['+ ' + overflow + ' more']));
    }

    // Cap warning when approaching limit
    let capWarning = null;
    if (subjectCount >= this.maxBatchSize) {
      capWarning = el('div', { class: 'bulk-cap-warning', role: 'alert' }, [
        'Batch operations are limited to ' + this.maxBatchSize + ' subjects per call. Select fewer or repeat in batches.',
      ]);
    }

    // Inline fields
    const inlineFieldEls = [];
    if (def && def.inlineFields) {
      for (const f of def.inlineFields) {
        const fid = 'bulk-field-' + f.key + '-' + Math.random().toString(36).slice(2);
        const input = el('input', { type: 'text', id: fid });
        if (f.required) input.setAttribute('aria-required', 'true');
        if (this.state.inlineData[f.key]) input.value = this.state.inlineData[f.key];
        input.addEventListener('input', (e) => {
          this.state.inlineData[f.key] = e.target.value;
          this.refreshConfirmButton();
        });
        inlineFieldEls.push(
          el('div', { class: 'action-panel-row' }, [
            el('label', { for: fid, class: 'action-panel-label' }, [
              f.label + (f.required ? ' (required)' : ' (optional)'),
            ]),
            input,
          ]),
        );
      }
    }

    // Rationale
    const rationaleId = 'bulk-rationale-' + Math.random().toString(36).slice(2);
    const rationaleEl = el('textarea', {
      id: rationaleId,
      'aria-required': 'true',
      rows: '4',
      class: 'action-panel-rationale',
    });
    rationaleEl.value = this.state.rationale;
    rationaleEl.addEventListener('input', (e) => {
      this.state.rationale = e.target.value;
      this.refreshConfirmButton();
    });

    // Confirm button — count in label per §6.4
    const confirmText = 'Confirm ' + subjectCount + ' action' + (subjectCount === 1 ? '' : 's');
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

    // Status region
    const statusEl = el('div', {
      class: 'action-panel-status',
      'aria-live': 'polite',
      role: 'status',
    });
    if (this.state.error) {
      statusEl.textContent = 'Error: ' + this.state.error;
      statusEl.classList.add('action-panel-status-error');
    } else if (this.state.result) {
      const r = this.state.result;
      let txt = 'Affected ' + (r.affectedCount || 0) + ' subject(s)';
      if (Array.isArray(r.skipped) && r.skipped.length > 0) {
        txt += ', ' + r.skipped.length + ' skipped';
      }
      statusEl.textContent = txt;
    }
    this._statusEl = statusEl;

    const formChildren = [
      el('h3', { class: 'action-panel-title' }, [
        'Bulk action: ' + subjectCount + ' subject' + (subjectCount === 1 ? '' : 's'),
      ]),
      el('div', { class: 'action-panel-row' }, [
        el('label', { for: actionSelectId, class: 'action-panel-label' }, ['Action']),
        actionSelect,
      ]),
      el('div', { class: 'action-panel-row' }, [
        el(
          'div',
          { class: 'action-panel-label' },
          ['Subjects (' + subjectCount + ')'],
        ),
        subjectList,
      ]),
    ];
    inlineFieldEls.forEach((f) => formChildren.push(f));
    formChildren.push(
      el('div', { class: 'action-panel-row' }, [
        el('label', { for: rationaleId, class: 'action-panel-label' }, [
          'Rationale (applies to all)',
        ]),
        rationaleEl,
      ]),
    );
    if (capWarning) formChildren.push(capWarning);
    formChildren.push(statusEl);
    formChildren.push(
      el('div', { class: 'action-panel-buttons' }, [cancelBtn, confirmBtn]),
    );

    this.dom = el(
      'div',
      { class: 'action-panel-card', role: 'group', 'aria-label': 'Bulk action panel' },
      formChildren,
    );
    this.container.appendChild(this.dom);
    this.refreshConfirmButton();
  };

  BulkActionPanel.prototype.refreshConfirmButton = function () {
    if (!this._confirmBtn) return;
    const reason = this.validityReason();
    if (reason) {
      this._confirmBtn.setAttribute('disabled', 'disabled');
      this._confirmBtn.setAttribute('aria-disabled', 'true');
      this._confirmBtn.setAttribute('title', reason);
    } else {
      this._confirmBtn.removeAttribute('disabled');
      this._confirmBtn.removeAttribute('aria-disabled');
      this._confirmBtn.removeAttribute('title');
    }
  };

  BulkActionPanel.prototype.validityReason = function () {
    if (this.state.submitting) return 'Submitting…';
    if (!this.state.action) return 'Select an action';
    const def = BATCH_ACTIONS[this.state.action];
    if (!def) return 'Unknown action';
    if (this.subjects.length === 0) return 'No subjects selected';
    if (this.subjects.length > this.maxBatchSize) {
      return 'Too many subjects (max ' + this.maxBatchSize + ')';
    }
    // Subject-type compat — every subject must be valid for this action
    for (const s of this.subjects) {
      const t = subjectType(s);
      if (t && def.validSubjectTypes.indexOf(t) === -1) {
        return def.label + ' does not apply to ' + t + ' subjects';
      }
    }
    if (!(this.state.rationale && this.state.rationale.trim())) {
      return 'Rationale is required';
    }
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
    return null;
  };

  BulkActionPanel.prototype.submit = async function () {
    const reason = this.validityReason();
    if (reason) {
      this.state.error = reason;
      this.render();
      return;
    }
    const def = BATCH_ACTIONS[this.state.action];
    this.state.submitting = true;
    this.state.error = null;
    this.refreshConfirmButton();
    try {
      const payload = def.buildPayload(this.subjects, this.state.rationale, this.state.inlineData);
      let result;
      if (this.onConfirm) {
        result = await this.onConfirm(this.state.action, this.state.rationale, payload);
      } else {
        result = await global.AuroraCapabilities.callEndpoint(def.feature, payload);
      }
      this.state.submitting = false;
      this.state.result = result;
      this.render();
      return result;
    } catch (e) {
      this.state.submitting = false;
      this.state.error = e && e.message ? e.message : String(e);
      this.render();
      throw e;
    }
  };

  global.BulkActionPanel = BulkActionPanel;
  global.BulkActionPanel.BATCH_ACTIONS = BATCH_ACTIONS;
  global.BulkActionPanel.MAX_BATCH_SIZE = MAX_BATCH_SIZE;
})(window);
