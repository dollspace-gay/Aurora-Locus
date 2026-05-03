// StatusBadge substrate primitive (substrate primitive 12) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.12.
//
// Small inline badge showing a status (active, suspended, takedown,
// pending, deactivated, verified, pre-chain). The class controls
// color via tokens; the label routes through i18n when keys exist,
// falling back to the raw string.

(function (global) {
  'use strict';

  const VARIANT_FOR = {
    active: 'status-active',
    suspended: 'status-suspended',
    takedown: 'status-takedown',
    'taken-down': 'status-takedown',
    deactivated: 'status-deactivated',
    pending: 'status-pending',
    verified: 'status-verified',
    'pre-chain': 'status-pre-chain',
    open: 'status-pending',
    resolved: 'status-active',
    approved: 'status-active',
    denied: 'status-takedown',
    escalated: 'status-suspended',
    under_review: 'status-pending',
  };

  function classFor(status) {
    if (!status) return 'status-active';
    return VARIANT_FOR[status] || 'status-pending';
  }

  function render(status, label) {
    const text = label != null ? label : (status || '');
    const safe = global.AuroraDom ? global.AuroraDom.esc(text) : String(text);
    return '<span class="status-badge ' + classFor(status) + '">' + safe + '</span>';
  }

  global.AuroraStatusBadge = {
    render: render,
    classFor: classFor,
  };
})(window);
