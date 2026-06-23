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
      '<div id="mod-lexicon-banner" style="display:none;"></div>' +
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
      // §5.5.4 Phase B — reviewer assignment (§4).
      '  <div class="settings-card">' +
      '    <h3>Reviewer assignment <span class="role-tag">SuperAdmin only</span></h3>' +
      '    <p class="settings-help">How new queue items are routed to operators, in the <strong>Full</strong> tier only. Changing the mode re-shows the mode-change banner for every operator.</p>' +
      '    <div id="mod-reviewer-banner" class="settings-help" style="display:none; padding:0.4rem; border-left:3px solid #c90;"></div>' +
      '    <fieldset>' +
      '      <label style="display:block;">Assignment mode' +
      '        <select id="mod-reviewer-mode" style="display:block; margin-top:0.25rem;"' + (isSuper ? '' : ' disabled') + '>' +
      '          <option value="manual">Manual (no auto-assignment)</option>' +
      '          <option value="round-robin">Round-robin (rotate through operators)</option>' +
      '          <option value="load-balanced">Load-balanced (fewest active items)</option>' +
      '          <option value="category-routed">By report category (use the map below)</option>' +
      '        </select></label>' +
      '      <div id="mod-reviewer-map" style="display:none; margin-top:0.75rem;">' +
      '        <p class="settings-help">Per-category operator pool — comma-separated DIDs. A category with an empty pool is left unassigned (warning shown).</p>' +
      reviewerMapRows(isSuper) +
      '        <div id="mod-reviewer-empty-warn" class="settings-help" style="color:#c00;"></div>' +
      '      </div>' +
      '      <label style="display:block; margin-top:0.5rem;">Rationale (required)' +
      '        <textarea id="mod-reviewer-rationale" rows="2" style="width:100%;"' + (isSuper ? '' : ' disabled') + '></textarea></label>' +
      '      <p>Current: <strong id="mod-reviewer-current">Loading…</strong></p>' +
      (isSuper ? '<button type="button" class="btn-primary" id="mod-reviewer-save">Save assignment</button>' : '<p class="settings-help">SuperAdmin role required to change reviewer assignment.</p>') +
      '    </fieldset>' +
      '  </div>' +
      // §5.5.4 Phase C — auto-label rules (§3).
      '  <div class="settings-card">' +
      '    <h3>Auto-label rules <span class="role-tag">SuperAdmin only</span></h3>' +
      '    <p class="settings-help">Rules that auto-apply a label when a subject accrues reports, operator actions, or (for new accounts) posts — in the <strong>Full</strong> tier only. Max 100 active.</p>' +
      '    <label><input type="checkbox" id="mod-rules-show-deleted"> Show deleted rules</label>' +
      '    <div id="mod-rules-list" style="margin:0.5rem 0;">Loading…</div>' +
      (isSuper ?
        '    <fieldset><legend id="mod-rule-form-legend">Add rule</legend>' +
        '      <input type="hidden" id="mod-rule-edit-id" value="">' +
        '      <label style="display:block;">Trigger' +
        '        <select id="mod-rule-trigger-type" style="display:block; margin-top:0.2rem;">' +
        '          <option value="report-count">report-count</option>' +
        '          <option value="operator-action">operator-action</option>' +
        '          <option value="account-age-activity">account-age-activity</option>' +
        '        </select></label>' +
        '      <div id="mod-rule-params-report-count" class="mod-rule-params">' +
        '        <label>category ' + categorySelect('mod-rule-rc-category') + '</label>' +
        '        <label> threshold <input type="number" id="mod-rule-rc-threshold" min="1" style="width:5rem;"></label>' +
        '        <label> window_days <input type="number" id="mod-rule-rc-window" min="1" max="365" style="width:5rem;"></label>' +
        '      </div>' +
        '      <div id="mod-rule-params-operator-action" class="mod-rule-params" style="display:none;">' +
        '        <label>action_type ' + actionTypeSelect('mod-rule-oa-action') + '</label>' +
        '        <label> threshold <input type="number" id="mod-rule-oa-threshold" min="1" style="width:5rem;"></label>' +
        '        <label> window_days <input type="number" id="mod-rule-oa-window" min="1" max="365" style="width:5rem;"></label>' +
        '      </div>' +
        '      <div id="mod-rule-params-account-age-activity" class="mod-rule-params" style="display:none;">' +
        '        <label>max_age_days <input type="number" id="mod-rule-aa-maxage" min="1" max="365" style="width:5rem;"></label>' +
        '        <label> min_posts <input type="number" id="mod-rule-aa-minposts" min="1" style="width:5rem;"></label>' +
        '      </div>' +
        '      <label style="display:block; margin-top:0.4rem;">Label value <input type="text" id="mod-rule-label" placeholder="tools.aurora.ops.moderation.…" style="width:100%;"></label>' +
        '      <label style="display:block;">Subject scope' +
        '        <select id="mod-rule-scope" style="display:block; margin-top:0.2rem;">' +
        '          <option value="account">account</option><option value="post">post</option><option value="both">both</option>' +
        '        </select></label>' +
        '      <label style="display:block;"><input type="checkbox" id="mod-rule-enabled" checked> Enabled</label>' +
        '      <label style="display:block;">Rationale <textarea id="mod-rule-rationale" rows="2" style="width:100%;"></textarea></label>' +
        '      <button type="button" class="btn-primary" id="mod-rule-save">Save rule</button>' +
        '      <button type="button" id="mod-rule-cancel" style="display:none;">Cancel edit</button>' +
        '    </fieldset>'
        : '    <p class="settings-help">SuperAdmin role required to manage auto-label rules.</p>') +
      '  </div>' +
      // §5.5.4 Phase D — escalation rules (§5).
      '  <div class="settings-card">' +
      '    <h3>Escalation rules <span class="role-tag">SuperAdmin only</span></h3>' +
      '    <p class="settings-help">Rules that auto-escalate a queue item (status → escalated) on severity signals — in the <strong>Full</strong> tier only. Max 100 active. De-escalate from the queue page.</p>' +
      '    <label><input type="checkbox" id="mod-esc-show-deleted"> Show deleted rules</label>' +
      '    <div id="mod-esc-list" style="margin:0.5rem 0;">Loading…</div>' +
      (isSuper ?
        '    <fieldset><legend id="mod-esc-form-legend">Add rule</legend>' +
        '      <input type="hidden" id="mod-esc-edit-id" value="">' +
        '      <label style="display:block;">Trigger' +
        '        <select id="mod-esc-trigger-type" style="display:block; margin-top:0.2rem;">' +
        '          <option value="report-count">report-count</option>' +
        '          <option value="operator-action">operator-action</option>' +
        '          <option value="category-match">category-match</option>' +
        '        </select></label>' +
        '      <div id="mod-esc-params-report-count" class="mod-esc-params">' +
        '        <label>category ' + categorySelect('mod-esc-rc-category') + '</label>' +
        '        <label> threshold <input type="number" id="mod-esc-rc-threshold" min="1" style="width:5rem;"></label>' +
        '        <label> window_days <input type="number" id="mod-esc-rc-window" min="1" max="365" style="width:5rem;"></label>' +
        '      </div>' +
        '      <div id="mod-esc-params-operator-action" class="mod-esc-params" style="display:none;">' +
        '        <label>action_type ' + actionTypeSelect('mod-esc-oa-action') + '</label>' +
        '        <label> threshold <input type="number" id="mod-esc-oa-threshold" min="1" style="width:5rem;"></label>' +
        '        <label> window_days <input type="number" id="mod-esc-oa-window" min="1" max="365" style="width:5rem;"></label>' +
        '      </div>' +
        '      <div id="mod-esc-params-category-match" class="mod-esc-params" style="display:none;">' +
        '        <label>category ' + categorySelect('mod-esc-cm-category') + '</label>' +
        '      </div>' +
        '      <label style="display:block; margin-top:0.4rem;">Action' +
        '        <select id="mod-esc-action" style="display:block; margin-top:0.2rem;">' +
        '          <option value="mark">mark (escalate in place)</option>' +
        '          <option value="reassign-to-superadmin">reassign-to-superadmin</option>' +
        '        </select></label>' +
        '      <label style="display:block;"><input type="checkbox" id="mod-esc-enabled" checked> Enabled</label>' +
        '      <label style="display:block;">Rationale <textarea id="mod-esc-rationale" rows="2" style="width:100%;"></textarea></label>' +
        '      <button type="button" class="btn-primary" id="mod-esc-save">Save rule</button>' +
        '      <button type="button" id="mod-esc-cancel" style="display:none;">Cancel edit</button>' +
        '    </fieldset>'
        : '    <p class="settings-help">SuperAdmin role required to manage escalation rules.</p>') +
      '  </div>' +
      '</div>';

    await loadLexiconBanner();
    await loadModerationMode();
    await loadModerationDefaults();
    await loadReviewerAssignment();
    await loadAutoLabelRules();
    await loadEscalationRules();
    if (isSuper) {
      const escTrig = document.getElementById('mod-esc-trigger-type');
      if (escTrig) escTrig.addEventListener('change', syncEscParamsVisibility);
      const escSave = document.getElementById('mod-esc-save');
      if (escSave) escSave.addEventListener('click', saveEscalationRule);
      const escCancel = document.getElementById('mod-esc-cancel');
      if (escCancel) escCancel.addEventListener('click', resetEscForm);
    }
    {
      const escShowDel = document.getElementById('mod-esc-show-deleted');
      if (escShowDel) escShowDel.addEventListener('change', loadEscalationRules);
    }
    if (isSuper) {
      const saveBtn = document.getElementById('mod-mode-save');
      if (saveBtn) saveBtn.addEventListener('click', saveModerationMode);
      const defSave = document.getElementById('mod-defaults-save');
      if (defSave) defSave.addEventListener('click', saveModerationDefaults);
      const actionSel = document.getElementById('mod-default-action');
      if (actionSel) actionSel.addEventListener('change', syncCategoryMapVisibility);
      const revSave = document.getElementById('mod-reviewer-save');
      if (revSave) revSave.addEventListener('click', saveReviewerAssignment);
      const revMode = document.getElementById('mod-reviewer-mode');
      if (revMode) revMode.addEventListener('change', syncReviewerMapVisibility);
      document.querySelectorAll('.mod-reviewer-pool').forEach(function (el) {
        el.addEventListener('input', syncReviewerEmptyWarning);
      });
      const trig = document.getElementById('mod-rule-trigger-type');
      if (trig) trig.addEventListener('change', syncRuleParamsVisibility);
      const ruleSave = document.getElementById('mod-rule-save');
      if (ruleSave) ruleSave.addEventListener('click', saveAutoLabelRule);
      const ruleCancel = document.getElementById('mod-rule-cancel');
      if (ruleCancel) ruleCancel.addEventListener('click', resetRuleForm);
    }
    const showDel = document.getElementById('mod-rules-show-deleted');
    if (showDel) showDel.addEventListener('change', loadAutoLabelRules);
    return {};
  }

  // The 16 emit_event moderation action_types (operator-action trigger).
  var OPERATOR_ACTION_TYPES = ['TakedownAccount', 'SuspendAccount', 'RestoreAccount', 'DeleteAccount', 'ApplyLabel', 'RemoveLabel', 'TakedownRecord', 'QuarantineBlob', 'RestoreBlob', 'DeleteBlob', 'ResolveReport', 'DismissReport', 'ResolveAppeal', 'EscalateAppeal', 'SendEmail', 'UpdateSubjectStatus'];

  function categorySelect(id) {
    return '<select id="' + id + '">' + REPORT_CATEGORIES.map(function (c) {
      return '<option value="' + c + '">' + c + '</option>';
    }).join('') + '</select>';
  }
  function actionTypeSelect(id) {
    return '<select id="' + id + '">' + OPERATOR_ACTION_TYPES.map(function (a) {
      return '<option value="' + a + '">' + a + '</option>';
    }).join('') + '</select>';
  }

  function syncRuleParamsVisibility() {
    const t = document.getElementById('mod-rule-trigger-type').value;
    document.querySelectorAll('.mod-rule-params').forEach(function (el) { el.style.display = 'none'; });
    const active = document.getElementById('mod-rule-params-' + t);
    if (active) active.style.display = 'block';
  }

  function reviewerMapRows(isSuper) {
    return REPORT_CATEGORIES.map(function (cat) {
      return '<label style="display:block; margin-top:0.25rem;">' + esc(cat) +
        '          <input type="text" data-category="' + esc(cat) + '" class="mod-reviewer-pool" placeholder="did:plc:…, did:plc:…" style="display:block; width:100%; margin-top:0.15rem;"' + (isSuper ? '' : ' disabled') + '></label>';
    }).join('');
  }

  function syncReviewerMapVisibility() {
    const sel = document.getElementById('mod-reviewer-mode');
    const map = document.getElementById('mod-reviewer-map');
    if (sel && map) map.style.display = sel.value === 'category-routed' ? 'block' : 'none';
    syncReviewerEmptyWarning();
  }

  // Parse a comma-separated DID input into a trimmed, non-empty array.
  function parsePool(text) {
    return (text || '').split(',').map(function (s) { return s.trim(); }).filter(function (s) { return s.length > 0; });
  }

  function syncReviewerEmptyWarning() {
    const warn = document.getElementById('mod-reviewer-empty-warn');
    if (!warn) return;
    const empties = [];
    document.querySelectorAll('.mod-reviewer-pool').forEach(function (el) {
      if (parsePool(el.value).length === 0) empties.push(el.getAttribute('data-category'));
    });
    warn.textContent = empties.length ? ('Empty pool (left unassigned): ' + empties.join(', ')) : '';
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

  async function loadReviewerAssignment() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.defaults.reviewer-assignment-mode');
      const mode = (data && typeof data.value === 'string') ? data.value : 'manual';
      const sel = document.getElementById('mod-reviewer-mode');
      if (sel) sel.value = mode;
      const cur = document.getElementById('mod-reviewer-current');
      if (cur) cur.textContent = mode + settingSourceSuffix(data && data.source);
    } catch (e) { /* ignore */ }
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.defaults.reviewer-routing-category-map');
      const map = (data && data.value && typeof data.value === 'object') ? data.value : {};
      document.querySelectorAll('.mod-reviewer-pool').forEach(function (el) {
        const arr = map[el.getAttribute('data-category')];
        el.value = Array.isArray(arr) ? arr.join(', ') : '';
      });
    } catch (e) { /* ignore */ }
    // Mode-change banner: show when the substrate version is newer than the
    // per-operator localStorage dismissal (§4.5).
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.defaults.reviewer-mode-version');
      const version = (data && typeof data.value === 'number') ? data.value : 0;
      const banner = document.getElementById('mod-reviewer-banner');
      const dismissedKey = 'aurora.banner-dismissed.queue-assignment-mode-change.v' + version;
      if (banner && version > 0 && !localStorage.getItem(dismissedKey)) {
        banner.textContent = 'Reviewer-assignment mode changed (v' + version + '). New items route per the current mode. [dismiss]';
        banner.style.display = 'block';
        banner.style.cursor = 'pointer';
        banner.onclick = function () { localStorage.setItem(dismissedKey, '1'); banner.style.display = 'none'; };
      }
    } catch (e) { /* ignore */ }
    syncReviewerMapVisibility();
  }

  async function saveReviewerAssignment() {
    const ep = global.AuroraEndpoints;
    const mode = document.getElementById('mod-reviewer-mode').value;
    const rationale = document.getElementById('mod-reviewer-rationale').value.trim();
    if (!rationale) { global.AuroraToast.warning('Rationale is required.'); return; }
    const map = {};
    document.querySelectorAll('.mod-reviewer-pool').forEach(function (el) {
      const pool = parsePool(el.value);
      if (pool.length > 0) map[el.getAttribute('data-category')] = pool;
    });
    if (mode === 'category-routed' && Object.keys(map).length === 0) {
      global.AuroraToast.warning('By-category mode needs at least one category with an operator pool.');
      return;
    }
    try {
      // Write the map first, then the mode (the mode write bumps the version
      // server-side and lands the most-recent audit entry).
      await ep.admin.setRuntimeSetting({ key: 'moderation.defaults.reviewer-routing-category-map', value: map, rationale: rationale });
      const res = await ep.admin.setRuntimeSetting({ key: 'moderation.defaults.reviewer-assignment-mode', value: mode, rationale: rationale });
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success('Reviewer assignment saved.', auditEntryId ? {
        action: { label: 'View audit entry', href: '#mod/audit/' + encodeURIComponent(auditEntryId) },
      } : undefined);
      await loadReviewerAssignment();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function ruleSummary(r) {
    const p = r.triggerParams || {};
    if (r.triggerType === 'report-count') return 'report-count: ' + p.category + ' ≥' + p.threshold + ' / ' + p.window_days + 'd';
    if (r.triggerType === 'operator-action') return 'operator-action: ' + p.action_type + ' ≥' + p.threshold + ' / ' + p.window_days + 'd';
    if (r.triggerType === 'account-age-activity') return 'account-age-activity: <' + p.max_age_days + 'd, ≥' + p.min_posts + ' posts';
    return r.triggerType;
  }

  async function loadAutoLabelRules() {
    const ep = global.AuroraEndpoints;
    const list = document.getElementById('mod-rules-list');
    if (!ep || !list) return;
    const includeDeleted = !!(document.getElementById('mod-rules-show-deleted') || {}).checked;
    try {
      const data = await ep.admin.listAutoLabelRules({ includeDeleted: includeDeleted });
      const rules = (data && data.rules) || [];
      if (!rules.length) { list.textContent = 'No auto-label rules.'; return; }
      list.innerHTML = rules.map(function (r) {
        const deleted = r.deletedAt ? ' <em>(deleted)</em>' : '';
        const state = r.enabled ? 'enabled' : 'disabled';
        return '<div class="mod-rule-row" style="border-bottom:1px solid #ddd; padding:0.3rem 0;">' +
          '<strong>' + esc(r.labelValue) + '</strong> — ' + esc(ruleSummary(r)) + ' [' + esc(r.subjectScope) + ', ' + state + ']' + deleted +
          (r.deletedAt ? '' :
            ' <button type="button" class="mod-rule-edit" data-id="' + esc(r.id) + '">Edit</button>' +
            ' <button type="button" class="mod-rule-del" data-id="' + esc(r.id) + '">Delete</button>') +
          '</div>';
      }).join('');
      list.querySelectorAll('.mod-rule-del').forEach(function (b) {
        b.addEventListener('click', function () { deleteAutoLabelRule(b.getAttribute('data-id')); });
      });
      list.querySelectorAll('.mod-rule-edit').forEach(function (b) {
        b.addEventListener('click', function () { editAutoLabelRule(b.getAttribute('data-id'), rules); });
      });
    } catch (e) { list.textContent = 'Failed to load rules.'; }
  }

  function ruleParamsFromForm(triggerType) {
    if (triggerType === 'report-count') {
      return {
        category: document.getElementById('mod-rule-rc-category').value,
        threshold: parseInt(document.getElementById('mod-rule-rc-threshold').value, 10),
        window_days: parseInt(document.getElementById('mod-rule-rc-window').value, 10),
      };
    }
    if (triggerType === 'operator-action') {
      return {
        action_type: document.getElementById('mod-rule-oa-action').value,
        threshold: parseInt(document.getElementById('mod-rule-oa-threshold').value, 10),
        window_days: parseInt(document.getElementById('mod-rule-oa-window').value, 10),
      };
    }
    return {
      max_age_days: parseInt(document.getElementById('mod-rule-aa-maxage').value, 10),
      min_posts: parseInt(document.getElementById('mod-rule-aa-minposts').value, 10),
    };
  }

  function resetRuleForm() {
    document.getElementById('mod-rule-edit-id').value = '';
    document.getElementById('mod-rule-form-legend').textContent = 'Add rule';
    document.getElementById('mod-rule-cancel').style.display = 'none';
    document.getElementById('mod-rule-label').value = '';
    document.getElementById('mod-rule-rationale').value = '';
  }

  function editAutoLabelRule(id, rules) {
    const r = rules.filter(function (x) { return x.id === id; })[0];
    if (!r) return;
    document.getElementById('mod-rule-edit-id').value = id;
    document.getElementById('mod-rule-form-legend').textContent = 'Edit rule';
    document.getElementById('mod-rule-cancel').style.display = '';
    document.getElementById('mod-rule-trigger-type').value = r.triggerType;
    syncRuleParamsVisibility();
    const p = r.triggerParams || {};
    if (r.triggerType === 'report-count') {
      document.getElementById('mod-rule-rc-category').value = p.category;
      document.getElementById('mod-rule-rc-threshold').value = p.threshold;
      document.getElementById('mod-rule-rc-window').value = p.window_days;
    } else if (r.triggerType === 'operator-action') {
      document.getElementById('mod-rule-oa-action').value = p.action_type;
      document.getElementById('mod-rule-oa-threshold').value = p.threshold;
      document.getElementById('mod-rule-oa-window').value = p.window_days;
    } else {
      document.getElementById('mod-rule-aa-maxage').value = p.max_age_days;
      document.getElementById('mod-rule-aa-minposts').value = p.min_posts;
    }
    document.getElementById('mod-rule-label').value = r.labelValue;
    document.getElementById('mod-rule-scope').value = r.subjectScope;
    document.getElementById('mod-rule-enabled').checked = r.enabled;
  }

  async function saveAutoLabelRule() {
    const ep = global.AuroraEndpoints;
    const triggerType = document.getElementById('mod-rule-trigger-type').value;
    const label = document.getElementById('mod-rule-label').value.trim();
    if (!label) { global.AuroraToast.warning('Label value is required.'); return; }
    const rationale = document.getElementById('mod-rule-rationale').value.trim();
    const body = {
      triggerType: triggerType,
      triggerParams: ruleParamsFromForm(triggerType),
      labelValue: label,
      subjectScope: document.getElementById('mod-rule-scope').value,
      enabled: document.getElementById('mod-rule-enabled').checked,
      rationale: rationale,
    };
    const editId = document.getElementById('mod-rule-edit-id').value;
    try {
      if (editId) { body.id = editId; await ep.admin.editAutoLabelRule(body); }
      else { await ep.admin.createAutoLabelRule(body); }
      global.AuroraToast.success('Auto-label rule saved.');
      resetRuleForm();
      await loadAutoLabelRules();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function deleteAutoLabelRule(id) {
    const confirmResult = await global.AuroraModal.destructiveConfirm({
      heading: 'Delete auto-label rule',
      body: 'Soft-delete this rule? It stops firing immediately; history is retained.',
      confirmLabel: 'Delete rule',
    });
    if (!confirmResult.confirmed) return;
    try {
      await global.AuroraEndpoints.admin.deleteAutoLabelRule({ id: id });
      global.AuroraToast.success('Rule deleted.');
      await loadAutoLabelRules();
    } catch (e) {
      global.AuroraToast.danger('Delete failed: ' + (e && e.message ? e.message : ''));
    }
  }

  function syncEscParamsVisibility() {
    const t = document.getElementById('mod-esc-trigger-type').value;
    document.querySelectorAll('.mod-esc-params').forEach(function (el) { el.style.display = 'none'; });
    const active = document.getElementById('mod-esc-params-' + t);
    if (active) active.style.display = 'block';
  }

  function escSummary(r) {
    const p = r.triggerParams || {};
    let trig = r.triggerType;
    if (r.triggerType === 'report-count') trig = 'report-count: ' + p.category + ' ≥' + p.threshold + ' / ' + p.window_days + 'd';
    else if (r.triggerType === 'operator-action') trig = 'operator-action: ' + p.action_type + ' ≥' + p.threshold + ' / ' + p.window_days + 'd';
    else if (r.triggerType === 'category-match') trig = 'category-match: ' + p.category;
    return trig + ' → ' + r.actionType;
  }

  async function loadEscalationRules() {
    const ep = global.AuroraEndpoints;
    const list = document.getElementById('mod-esc-list');
    if (!ep || !list) return;
    const includeDeleted = !!(document.getElementById('mod-esc-show-deleted') || {}).checked;
    try {
      const data = await ep.admin.listEscalationRules({ includeDeleted: includeDeleted });
      const rules = (data && data.rules) || [];
      if (!rules.length) { list.textContent = 'No escalation rules.'; return; }
      list.innerHTML = rules.map(function (r) {
        const deleted = r.deletedAt ? ' <em>(deleted)</em>' : '';
        const state = r.enabled ? 'enabled' : 'disabled';
        return '<div class="mod-esc-row" style="border-bottom:1px solid #ddd; padding:0.3rem 0;">' +
          esc(escSummary(r)) + ' [' + state + ']' + deleted +
          (r.deletedAt ? '' :
            ' <button type="button" class="mod-esc-edit" data-id="' + esc(r.id) + '">Edit</button>' +
            ' <button type="button" class="mod-esc-del" data-id="' + esc(r.id) + '">Delete</button>') +
          '</div>';
      }).join('');
      list.querySelectorAll('.mod-esc-del').forEach(function (b) {
        b.addEventListener('click', function () { deleteEscalationRule(b.getAttribute('data-id')); });
      });
      list.querySelectorAll('.mod-esc-edit').forEach(function (b) {
        b.addEventListener('click', function () { editEscalationRule(b.getAttribute('data-id'), rules); });
      });
    } catch (e) { list.textContent = 'Failed to load rules.'; }
  }

  function escParamsFromForm(triggerType) {
    if (triggerType === 'report-count') {
      return {
        category: document.getElementById('mod-esc-rc-category').value,
        threshold: parseInt(document.getElementById('mod-esc-rc-threshold').value, 10),
        window_days: parseInt(document.getElementById('mod-esc-rc-window').value, 10),
      };
    }
    if (triggerType === 'operator-action') {
      return {
        action_type: document.getElementById('mod-esc-oa-action').value,
        threshold: parseInt(document.getElementById('mod-esc-oa-threshold').value, 10),
        window_days: parseInt(document.getElementById('mod-esc-oa-window').value, 10),
      };
    }
    return { category: document.getElementById('mod-esc-cm-category').value };
  }

  function resetEscForm() {
    document.getElementById('mod-esc-edit-id').value = '';
    document.getElementById('mod-esc-form-legend').textContent = 'Add rule';
    document.getElementById('mod-esc-cancel').style.display = 'none';
    document.getElementById('mod-esc-rationale').value = '';
  }

  function editEscalationRule(id, rules) {
    const r = rules.filter(function (x) { return x.id === id; })[0];
    if (!r) return;
    document.getElementById('mod-esc-edit-id').value = id;
    document.getElementById('mod-esc-form-legend').textContent = 'Edit rule';
    document.getElementById('mod-esc-cancel').style.display = '';
    document.getElementById('mod-esc-trigger-type').value = r.triggerType;
    syncEscParamsVisibility();
    const p = r.triggerParams || {};
    if (r.triggerType === 'report-count') {
      document.getElementById('mod-esc-rc-category').value = p.category;
      document.getElementById('mod-esc-rc-threshold').value = p.threshold;
      document.getElementById('mod-esc-rc-window').value = p.window_days;
    } else if (r.triggerType === 'operator-action') {
      document.getElementById('mod-esc-oa-action').value = p.action_type;
      document.getElementById('mod-esc-oa-threshold').value = p.threshold;
      document.getElementById('mod-esc-oa-window').value = p.window_days;
    } else {
      document.getElementById('mod-esc-cm-category').value = p.category;
    }
    document.getElementById('mod-esc-action').value = r.actionType;
    document.getElementById('mod-esc-enabled').checked = r.enabled;
  }

  async function saveEscalationRule() {
    const ep = global.AuroraEndpoints;
    const triggerType = document.getElementById('mod-esc-trigger-type').value;
    const rationale = document.getElementById('mod-esc-rationale').value.trim();
    const body = {
      triggerType: triggerType,
      triggerParams: escParamsFromForm(triggerType),
      actionType: document.getElementById('mod-esc-action').value,
      enabled: document.getElementById('mod-esc-enabled').checked,
      rationale: rationale,
    };
    const editId = document.getElementById('mod-esc-edit-id').value;
    try {
      if (editId) { body.id = editId; await ep.admin.editEscalationRule(body); }
      else { await ep.admin.createEscalationRule(body); }
      global.AuroraToast.success('Escalation rule saved.');
      resetEscForm();
      await loadEscalationRules();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function deleteEscalationRule(id) {
    const confirmResult = await global.AuroraModal.destructiveConfirm({
      heading: 'Delete escalation rule',
      body: 'Soft-delete this rule? It stops firing immediately; history is retained.',
      confirmLabel: 'Delete rule',
    });
    if (!confirmResult.confirmed) return;
    try {
      await global.AuroraEndpoints.admin.deleteEscalationRule({ id: id });
      global.AuroraToast.success('Rule deleted.');
      await loadEscalationRules();
    } catch (e) {
      global.AuroraToast.danger('Delete failed: ' + (e && e.message ? e.message : ''));
    }
  }

  // §5.5.4 Phase E — lexicon-migration banner (§6.4). Shows what a boot-time
  // report-category change migrated (pruned map keys + flagged rules) until
  // per-operator localStorage dismissal keyed on the migration timestamp.
  async function loadLexiconBanner() {
    const ep = global.AuroraEndpoints;
    const el = document.getElementById('mod-lexicon-banner');
    if (!ep || !el) return;
    try {
      const data = await ep.admin.getRuntimeSetting('moderation.lexicon.migration-banner');
      const raw = data && data.value;
      if (!raw || typeof raw !== 'string') return;
      const b = JSON.parse(raw);
      if (!b || !b.migratedAt) return;
      const dismissKey = 'aurora.banner-dismissed.lexicon-migration.' + b.migratedAt;
      if (localStorage.getItem(dismissKey)) return;
      const pruned = (b.prunedKeys || []).join(', ') || 'none';
      const flagged = (b.flaggedRuleIds || []).join(', ') || 'none';
      el.className = 'settings-help';
      el.style.cssText = 'display:block; padding:0.5rem; border-left:3px solid #c90; background:#fffbe6;';
      el.innerHTML = 'Report-category lexicon changed (' + esc(b.migratedAt) + '). Pruned map keys: ' + esc(pruned) +
        '. Flagged rules (review their category): ' + esc(flagged) + '. <button type="button" id="mod-lexicon-dismiss">Acknowledge</button>';
      const btn = document.getElementById('mod-lexicon-dismiss');
      if (btn) btn.addEventListener('click', function () { localStorage.setItem(dismissKey, '1'); el.style.display = 'none'; });
    } catch (e) { /* no banner */ }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configModerationPolicy', { mount: mount });
})(window);
