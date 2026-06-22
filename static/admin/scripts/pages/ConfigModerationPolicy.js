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
      // §5.5.4 Phase A — default action on report submission (§2),
      // its per-category override map (§2.3), and the stale-hold timeout
      // (§2.5). SuperAdmin-only; applies in the `full` tier (§2.7).
      '<hr class="config-section-divider">' +
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Default action on report submission <span class="role-tag">SuperAdmin only</span></h3>' +
      '    <p class="settings-help">Applied by substrate the moment a report lands, in the <strong>Full</strong> tier only. Operators confirm or override it during review.</p>' +
      '    <fieldset>' +
      '      <label style="display:block;">Default action' +
      '        <select id="mod-default-action" style="display:block; margin-top:0.25rem;"' + (isSuper ? '' : ' disabled') + '>' +
      '          <option value="acknowledge">Acknowledge (log only)</option>' +
      '          <option value="hide-pending-review">Hide pending review (apply hide-pending label)</option>' +
      '          <option value="auto-resolve-by-category">By report category (use the map below)</option>' +
      '        </select></label>' +
      '      <div id="mod-category-map" style="display:none; margin-top:0.75rem;">' +
      '        <p class="settings-help">Per-category action. Categories left at “acknowledge” may be omitted; an empty map falls back to acknowledge for all.</p>' +
      categoryMapRows(isSuper) +
      '      </div>' +
      '      <label style="display:block; margin-top:0.75rem;">Stale hide-pending hold (days)' +
      '        <input type="number" id="mod-stale-days" min="1" max="365" style="display:block; margin-top:0.25rem; width:8rem;"' + (isSuper ? '' : ' disabled') + '></label>' +
      '      <p class="settings-help">Hide-pending labels older than this are lazily auto-removed (1–365).</p>' +
      '      <label style="display:block; margin-top:0.5rem;">Rationale (required)' +
      '        <textarea id="mod-defaults-rationale" rows="2" style="width:100%;"' + (isSuper ? '' : ' disabled') + '></textarea></label>' +
      '      <p>Current: <strong id="mod-defaults-current">Loading…</strong></p>' +
      (isSuper ? '<button type="button" class="btn-primary" id="mod-defaults-save">Save defaults</button>' : '<p class="settings-help">SuperAdmin role required to change moderation defaults.</p>') +
      '    </fieldset>' +
      '  </div>' +
      '</div>';

    await loadModerationMode();
    await loadModerationDefaults();
    if (isSuper) {
      const saveBtn = document.getElementById('mod-mode-save');
      if (saveBtn) saveBtn.addEventListener('click', saveModerationMode);
      const defSave = document.getElementById('mod-defaults-save');
      if (defSave) defSave.addEventListener('click', saveModerationDefaults);
      const actionSel = document.getElementById('mod-default-action');
      if (actionSel) actionSel.addEventListener('change', syncCategoryMapVisibility);
    }
    return {};
  }

  // The six report categories (ReportReason vocabulary). Map values are
  // acknowledge | hide-pending-review; "acknowledge" doubles as "unset".
  var REPORT_CATEGORIES = ['spam', 'violation', 'misleading', 'sexual', 'rude', 'other'];

  function categoryMapRows(isSuper) {
    return REPORT_CATEGORIES.map(function (cat) {
      return '<label style="display:block; margin-top:0.25rem;">' + esc(cat) +
        '          <select data-category="' + esc(cat) + '" class="mod-cat-action" style="margin-left:0.5rem;"' + (isSuper ? '' : ' disabled') + '>' +
        '            <option value="acknowledge">acknowledge</option>' +
        '            <option value="hide-pending-review">hide-pending-review</option>' +
        '          </select></label>';
    }).join('');
  }

  function syncCategoryMapVisibility() {
    const sel = document.getElementById('mod-default-action');
    const map = document.getElementById('mod-category-map');
    if (sel && map) map.style.display = sel.value === 'auto-resolve-by-category' ? 'block' : 'none';
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

  async function loadModerationDefaults() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    let action = 'acknowledge';
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.defaults.report-action');
      if (data && typeof data.value === 'string') action = data.value;
      const sel = document.getElementById('mod-default-action');
      if (sel) sel.value = action;
      const cur = document.getElementById('mod-defaults-current');
      if (cur) cur.textContent = action + settingSourceSuffix(data && data.source);
    } catch (e) { /* ignore */ }
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.defaults.report-action-category-map');
      const map = (data && data.value && typeof data.value === 'object') ? data.value : {};
      document.querySelectorAll('.mod-cat-action').forEach(function (el) {
        const v = map[el.getAttribute('data-category')];
        el.value = (v === 'hide-pending-review') ? 'hide-pending-review' : 'acknowledge';
      });
    } catch (e) { /* ignore */ }
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.defaults.hide-pending-review-stale-days');
      const v = (data && typeof data.value === 'number') ? data.value : 90;
      const input = document.getElementById('mod-stale-days');
      if (input) input.value = v;
    } catch (e) { /* ignore */ }
    syncCategoryMapVisibility();
  }

  async function saveModerationDefaults() {
    const ep = global.AuroraEndpoints;
    const actionSel = document.getElementById('mod-default-action');
    const action = actionSel ? actionSel.value : 'acknowledge';
    const rationale = document.getElementById('mod-defaults-rationale').value.trim();
    if (!rationale) { global.AuroraToast.warning('Rationale is required.'); return; }
    const staleRaw = parseInt(document.getElementById('mod-stale-days').value, 10);
    if (!(staleRaw >= 1 && staleRaw <= 365)) {
      global.AuroraToast.warning('Stale-hold days must be 1–365.');
      return;
    }
    // Build the per-category map from non-acknowledge selections only —
    // acknowledge is the implicit default, so an all-acknowledge map is {}.
    const map = {};
    document.querySelectorAll('.mod-cat-action').forEach(function (el) {
      if (el.value === 'hide-pending-review') map[el.getAttribute('data-category')] = el.value;
    });
    if (action === 'auto-resolve-by-category' && Object.keys(map).length === 0) {
      global.AuroraToast.warning('By-category action needs at least one category set to hide-pending-review.');
      return;
    }
    try {
      await ep.admin.setRuntimeSetting({ key: 'moderation.defaults.report-action', value: action, rationale: rationale });
      await ep.admin.setRuntimeSetting({ key: 'moderation.defaults.report-action-category-map', value: map, rationale: rationale });
      // Last of the three writes lands the most-recent audit entry; link the toast to it.
      const res = await ep.admin.setRuntimeSetting({ key: 'moderation.defaults.hide-pending-review-stale-days', value: staleRaw, rationale: rationale });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Moderation defaults saved.', auditEntryId ? {
        action: { label: 'View audit entry', href: '#mod/audit/' + encodeURIComponent(auditEntryId) },
      } : undefined);
      await loadModerationDefaults();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configModerationPolicy', { mount: mount });
})(window);
