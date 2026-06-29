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
        const saveBtn = ev.target.closest('button[data-save="' + KEY_OPERATOR_SUPPLIED + '"]');
        if (saveBtn) {
          save(container);
          return;
        }
        if (ev.target.closest('#kr-run-migration-check')) {
          runMigrationCheck();
        }
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
      '</div>' +
      // Migration check (#376 / C4): read-only diagnostic confirming stored
      // signing keys align with what PLC publishes.
      '<div class="settings-card">' +
      '  <h3>' + esc(t('keyRotation.policy.migration_title')) + '</h3>' +
      '  <p class="settings-help">' + esc(t('keyRotation.policy.migration_help')) + '</p>' +
      '  <div class="form-actions">' +
      '    <button class="btn btn-secondary" id="kr-run-migration-check">' +
            esc(t('keyRotation.policy.migration_run')) + '</button>' +
      '  </div>' +
      '  <div id="kr-migration-result" role="status"></div>' +
      '</div>';
  }

  // Render the migration-check report inline: an empty-state pass message when
  // there are no divergences/unresolvables, else a per-account list.
  function renderMigrationReport(report) {
    const out = document.getElementById('kr-migration-result');
    if (!out) return;
    const divergences = (report && report.divergences) || [];
    const unresolvable = (report && report.unresolvable) || [];
    const checked = (report && report.accountsChecked) || 0;
    const aligned = (report && report.aligned) || 0;

    let html =
      '<p class="settings-help">' +
      esc(t('keyRotation.policy.migration_summary', {
        checked: checked, aligned: aligned,
        divergent: divergences.length, unresolvable: unresolvable.length,
      })) + '</p>';

    if (divergences.length === 0 && unresolvable.length === 0) {
      html +=
        '<div class="empty-state" role="status"><p>' +
        esc(t('keyRotation.policy.migration_pass')) + '</p></div>';
    } else {
      if (divergences.length) {
        html += '<h4>' + esc(t('keyRotation.policy.migration_divergent')) + '</h4><ul>';
        divergences.forEach(function (d) {
          html += '<li><code>' + esc(d.did) + '</code>: ' +
            esc(t('keyRotation.policy.migration_stored')) + ' <code>' + esc(d.storedPublicDidKey) +
            '</code> ' + esc(t('keyRotation.policy.migration_published')) + ' <code>' +
            esc(d.publishedPublicDidKey) + '</code></li>';
        });
        html += '</ul>';
      }
      if (unresolvable.length) {
        html += '<h4>' + esc(t('keyRotation.policy.migration_unresolvable')) + '</h4><ul>';
        unresolvable.forEach(function (u) {
          html += '<li><code>' + esc(u.did) + '</code>: ' + esc(u.reason) + '</li>';
        });
        html += '</ul>';
      }
    }
    out.innerHTML = html;
  }

  async function runMigrationCheck() {
    const out = document.getElementById('kr-migration-result');
    if (out) out.innerHTML = '<p class="settings-help">' + esc(t('common.loading')) + '</p>';
    try {
      const report = await global.AuroraClient.post(
        'tools.aurora.superadmin.runSigningKeyMigrationCheck', {});
      renderMigrationReport(report);
    } catch (e) {
      if (out) {
        out.innerHTML = '';
      }
      global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
    }
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
