// Configuration → Moderation policy page (route: #configuration/moderation-policy).
//
// Hosts the deployment-wide moderation-tier switch (full / reduced / disabled),
// moved here from UI & modes (#340) — "moderation policy" is the intuitive home
// for the tier, leaving UI & modes for theme/language/branding. SuperAdmin-only
// (route-gated). The §5.5.4 "configurable moderation defaults" surface is a
// separate work-thread (design contracts in progress) and renders as an honest
// in-development section below.
//
// NOTE (design divergence, flagged for reconciliation): the locked design
// §5.5.3 places `moderation-mode` on UI & modes and §5.5.4 reserves this page
// for "configurable moderation defaults". This move (operator IA decision,
// #340) puts the tier switch here; the design doc should be reconciled to match.
//
// What the mode does is admin-UI visibility (design §5.7.4 / routes.js
// domainMinRole): the tier governs which nav domains render per role × mode —
// it is not a substrate moderation-path short-circuit (that framing is the
// deferred external-delegation surface, not committed here).

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  // Source-tier suffix (Runtime/File/Default/RecoveryMode) via the shared
  // AuroraSourceTier primitive — same thin delegate UI & modes used.
  function settingSourceSuffix(source) {
    return global.AuroraSourceTier.suffix(source);
  }

  async function mount({ container }) {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Moderation policy</nav>' +
      '<header class="page-header"><div><h2>Moderation policy</h2>' +
      '<p class="page-subtitle">Deployment moderation tier and (soon) configurable defaults</p></div></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Moderation tier <span class="role-tag">SuperAdmin only</span></h3>' +
      '    <fieldset>' +
      '      <legend>Tier</legend>' +
      '      <label><input type="radio" name="mod-mode" value="full"' + (isSuper ? '' : ' disabled') + '> Full</label>' +
      '      <label style="margin-left: 0.75rem;"><input type="radio" name="mod-mode" value="reduced"' + (isSuper ? '' : ' disabled') + '> Reduced</label>' +
      '      <label style="margin-left: 0.75rem;"><input type="radio" name="mod-mode" value="disabled"' + (isSuper ? '' : ' disabled') + '> Disabled</label>' +
      '      <p>Current: <strong id="mod-mode-current">Loading…</strong></p>' +
      '      <ul class="settings-help" style="margin-top:0.5rem;">' +
      '        <li><strong>Full</strong> — complete on-PDS moderation; all moderation surfaces available.</li>' +
      '        <li><strong>Reduced</strong> — moderation surfaces (Queue, Reports, Appeals, Events) hidden; PDS management (Operations, Configuration, Kryphocron) stays available.</li>' +
      '        <li><strong>Disabled</strong> — only the Configuration domain is reachable; an emergency-access posture for every operator.</li>' +
      '      </ul>' +
      '      <label style="display: block; margin-top: 0.5rem;">Redirect URL (when disabled)' +
      '        <input type="text" id="mod-mode-redirect" style="width:100%;"' + (isSuper ? '' : ' disabled') + '></label>' +
      '      <label style="display: block; margin-top: 0.5rem;">Rationale (required)' +
      '        <textarea id="mod-mode-rationale" rows="2" style="width:100%;"' + (isSuper ? '' : ' disabled') + '></textarea></label>' +
      (isSuper ? '<button type="button" class="btn-primary" id="mod-mode-save">Save tier change</button>' : '<p class="settings-help">SuperAdmin role required to change the deployment-wide moderation tier.</p>') +
      '    </fieldset>' +
      '  </div>' +
      '</div>' +
      // §5.5.4 — configurable moderation defaults. Separate work-thread (design
      // contracts in progress); honest in-development surface, not a dead stub.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Configurable moderation defaults</h3>' +
      '  <p class="settings-help">Default moderation actions (e.g. new-account default status), auto-moderation rules, and reviewer-assignment policy. Design pass in progress — this section activates in a later release once the policy-defaults contract is locked.</p>' +
      '</section>';

    await loadModerationMode();
    if (isSuper) {
      const saveBtn = document.getElementById('mod-mode-save');
      if (saveBtn) saveBtn.addEventListener('click', saveModerationMode);
    }
    return {};
  }

  async function loadModerationMode() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.admin.getRuntimeSetting('moderation-mode');
      const value = (data && typeof data.value === 'string') ? data.value : 'full';
      const cur = document.getElementById('mod-mode-current');
      if (cur) cur.textContent = value + settingSourceSuffix(data && data.source);
      const radio = document.querySelector('input[name="mod-mode"][value="' + value + '"]');
      if (radio) radio.checked = true;
      if (global.AuroraSettings) global.AuroraSettings.setModerationModeCache(value);
    } catch (e) { /* ignore */ }
    try {
      const data = await ep.admin.getRuntimeSetting('moderation-mode-redirect-url');
      const v = (data && typeof data.value === 'string') ? data.value : '';
      const input = document.getElementById('mod-mode-redirect');
      if (input) input.value = v;
    } catch (e) { /* ignore */ }
  }

  async function saveModerationMode() {
    const selected = document.querySelector('input[name="mod-mode"]:checked');
    if (!selected) { global.AuroraToast.warning('Select a tier.'); return; }
    const rationale = document.getElementById('mod-mode-rationale').value.trim();
    if (!rationale) { global.AuroraToast.warning('Rationale is required.'); return; }
    // Switching to `disabled` locks every operator down to the Configuration
    // domain — high-impact, so it takes a typed-confirm with an explicit
    // warning. full/reduced take the standard destructive confirm.
    const toDisabled = selected.value === 'disabled';
    const confirmResult = await global.AuroraModal.destructiveConfirm(toDisabled ? {
      heading: 'Switch to disabled tier',
      body: 'Disabled tier restricts ALL operators to the Configuration domain only — moderation and operations surfaces disappear for everyone, SuperAdmin included. Re-enable from this page to restore access. Continue?',
      typedConfirmGate: 'DISABLE',
      confirmLabel: 'Switch to disabled',
    } : {
      heading: 'Switch moderation tier',
      body: 'Switch moderation tier to "' + selected.value + '"? This affects all operators using this PDS.',
      confirmLabel: 'Switch tier',
    });
    if (!confirmResult.confirmed) return;
    const redirect = document.getElementById('mod-mode-redirect').value.trim();
    try {
      await global.AuroraEndpoints.admin.setRuntimeSetting({ key: 'moderation-mode', value: selected.value, rationale: rationale });
      // Two setRuntimeSetting calls land two audit entries; link the toast to
      // the most-recent (redirect-url) entry per the last-entry rule.
      const res = await global.AuroraEndpoints.admin.setRuntimeSetting({ key: 'moderation-mode-redirect-url', value: redirect, rationale: rationale });
      const auditEntryId = res && res.auditEntryId;
      // The save updates the AuroraSettings cache via loadModerationMode below,
      // which app.js subscribes to and re-renders the sidebar from — so the nav
      // updates immediately (no "may").
      global.AuroraToast.success('Moderation tier saved. Navigation updated.', auditEntryId ? {
        action: {
          label: 'View audit entry',
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      await loadModerationMode();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configModerationPolicy', { mount: mount });
})(window);
