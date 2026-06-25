// Configuration → Key rotation policy (route: #configuration/key-rotation-policy).
// Key-rotation arc B2 (#373 / design §4.6). SuperAdmin-gated (route `requires:
// 'superadmin'`). A single live card toggling the operator-supplied-keys feature
// gate — the runtime setting `key_rotation.operator_supplied_keys_enabled`
// (bool, default OFF). The save routes through AuroraAuditedSave, so flipping
// the gate shows a destructive-confirm (rationale required) and lands a
// SetRuntimeSetting audit-chain entry.
//
// Why its own page (not folded into Kryphocron policy): kryphocron governs the
// at-rest CODEC layer (Laquna keys); this gates per-account PLC SIGNING-key
// rotation — a distinct domain. New rotation operator-settings (Phase C and
// beyond) accumulate here rather than on an unrelated page.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);

  // The single runtime-settings key this page owns (§4.6). A strict bool: the
  // backend rejects non-boolean values, so the checkbox sends a real bool.
  const KEY_OPERATOR_SUPPLIED = 'key_rotation.operator_supplied_keys_enabled';

  // Resolve the gate to its { value: bool, source } pair (§8.2.1 — the source
  // tier drives the per-card indicator). Tolerates the {value, source} shape
  // and a bare value; an absent/non-bool value reads as `false` (fail-closed).
  async function readGate() {
    try {
      const out = await global.AuroraEndpoints.admin.getRuntimeSetting(KEY_OPERATOR_SUPPLIED);
      const raw = out && (out.value !== undefined ? out.value : out);
      const source = out && out.source;
      const value = raw === true || raw === 'true';
      return { value: value, source: source || (raw == null ? 'Default' : 'Runtime') };
    } catch (e) {
      return { value: false, source: 'Default' };
    }
  }

  async function mount({ container }) {
    const session = global.AuroraSession;
    if (session && !session.hasRole('superadmin')) {
      container.innerHTML =
        '<header class="page-header"><h2>' + esc(t('keyRotation.policy.title')) + '</h2></header>' +
        '<div class="empty-state" role="status"><p>' +
        esc(t('errors.permissionDenied')) + '</p></div>';
      return {};
    }

    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#configuration/general">' + esc(t('settings.title')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('keyRotation.policy.title')) +
      '</nav>' +
      '<header class="page-header"><h2>' + esc(t('keyRotation.policy.title')) + '</h2>' +
      '  <p class="page-subtitle">' + esc(t('keyRotation.policy.subtitle')) + '</p></header>' +
      '<div class="settings-grid" id="krpolicy-grid">' +
      global.AuroraSkeleton.cards(1) +
      '</div>';

    await load(container);

    const grid = container.querySelector('#krpolicy-grid');
    if (grid) {
      grid.addEventListener('click', function (ev) {
        const btn = ev.target.closest('button[data-save="' + KEY_OPERATOR_SUPPLIED + '"]');
        if (btn) save(container);
      });
    }
    return {};
  }

  async function load(container) {
    const gate = await readGate();
    const grid = container.querySelector('#krpolicy-grid');
    if (!grid) return;

    grid.innerHTML =
      '<div class="settings-card" data-card="' + esc(KEY_OPERATOR_SUPPLIED) + '">' +
      '  <h3>' + esc(t('keyRotation.policy.operator_supplied_title')) +
          (gate.source ? ' ' + global.AuroraSourceTier.badge(gate.source) : '') + '</h3>' +
      '  <p class="settings-help">' + esc(t('keyRotation.policy.operator_supplied_help')) + '</p>' +
      '  <div class="form-group">' +
      '    <label class="checkbox-label">' +
      '      <input type="checkbox" id="kr-operator-supplied"' + (gate.value ? ' checked' : '') + '>' +
      '      ' + esc(t('keyRotation.policy.operator_supplied_label')) +
      '    </label>' +
      '  </div>' +
      '  <div class="form-actions">' +
      '    <button class="btn btn-primary" data-save="' + esc(KEY_OPERATOR_SUPPLIED) + '">' +
            esc(t('common.save')) + '</button>' +
      '  </div>' +
      '</div>';
  }

  async function save(container) {
    const el = document.getElementById('kr-operator-supplied');
    if (!el) return;
    const enabled = !!el.checked;
    // Flipping ON is the meaningful security decision (private keys travel in
    // request bodies; signals intent to use HSM-backed / pre-generated paths);
    // both directions are audit-chained. AuroraAuditedSave shows the confirm +
    // required rationale and writes the gate under that rationale.
    const r = await global.AuroraAuditedSave.run({
      heading: enabled
        ? t('keyRotation.policy.enable_heading')
        : t('keyRotation.policy.disable_heading'),
      body: enabled
        ? t('keyRotation.policy.enable_body')
        : t('keyRotation.policy.disable_body'),
      // Real JSON bool — the backend validator rejects "true"/1.
      settings: [{ key: KEY_OPERATOR_SUPPLIED, value: enabled }],
      successMessage: t('keyRotation.policy.save_success'),
    });
    // Refresh so the source-tier badge flips Default → Runtime, and so a
    // cancelled confirm reverts the checkbox to the stored state (§8.2.1).
    await load(container);
    return r;
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('configKeyRotationPolicy', { mount: mount });
  }
})(window);
