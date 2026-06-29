// AuroraAuditedSave — the canonical save-with-rationale flow (§8.2.2).
//
// High-impact settings (federation, moderation defaults, kryphocron policy,
// account permissions) must be saved with an operator rationale that lands in
// the audit chain. The rationale FIELD already lives in AuroraModal
// (rationaleRequired); what had drifted is the surrounding FLOW — open the
// rationale modal, write one-or-more runtime settings under that rationale,
// then surface a success toast linking the audit entry (or a danger toast on
// failure). This primitive consolidates that flow so every audited save is
// identical (it had been hand-rolled in ConfigThemes / ConfigKryphocronPolicy /
// ConfigUiModes / Laquna).
//
//   AuroraAuditedSave.run({
//     heading, body, confirmLabel?, typedConfirmGate?,
//     settings: [{ key, value }, ...],   // one or more runtime settings, all written under the one rationale
//     successMessage?,
//     cosmetic?, autoRationale?,         // see below
//   })  → Promise<{ saved: boolean, auditEntryId?: string, rationale?: string }>
//
// Returns { saved: false } if the operator cancels the modal or a write fails
// (a danger toast is shown on failure). On success, shows a success toast with
// a "View audit entry" action linking the last write's audit entry.
//
// `cosmetic: true` (#308) — for low-stakes operator preferences (e.g. the
// deployment-default theme). A required typed rationale is the right gate for
// destructive actions on durable state (rebuild / rotation / role grant) but is
// pure friction for "make this the default theme". Cosmetic mode keeps a light
// confirm (heading/body) but drops the required-rationale field and any
// typed-confirm gate, and writes the setting under an auto-filled rationale
// (`autoRationale`, default "cosmetic setting change") — so the change STILL
// lands an audit-chain entry (audit-trail completeness preserved), just without
// an operator-typed reason.

(function (global) {
  'use strict';

  function t(k, p) { return global.t ? global.t(k, p) : k; }

  async function run(spec) {
    spec = spec || {};
    const cosmetic = !!spec.cosmetic;
    const res = await global.AuroraModal.destructiveConfirm({
      heading: spec.heading,
      body: spec.body,
      rationaleRequired: !cosmetic,
      typedConfirmGate: cosmetic ? undefined : spec.typedConfirmGate,
      confirmLabel: spec.confirmLabel || t('common.save'),
    });
    if (!res.confirmed) return { saved: false };
    // Cosmetic settings auto-fill the rationale (no operator-typed reason) but
    // still emit the audit entry via setRuntimeSetting below.
    const rationale = cosmetic
      ? (spec.autoRationale || 'cosmetic setting change')
      : (res.rationale || '');
    let lastAudit = null;
    try {
      const settings = spec.settings || [];
      for (let i = 0; i < settings.length; i++) {
        const out = await global.AuroraEndpoints.admin.setRuntimeSetting({
          key: settings[i].key, value: settings[i].value, rationale: rationale,
        });
        if (out && out.auditEntryId) lastAudit = out.auditEntryId;
      }
      global.AuroraToast.success(
        spec.successMessage || t('common.save'),
        lastAudit
          ? { action: { label: t('settings.roles.view_audit'),
              href: '#mod/audit/' + encodeURIComponent(lastAudit) } }
          : undefined,
      );
      return { saved: true, auditEntryId: lastAudit, rationale: rationale };
    } catch (e) {
      global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      return { saved: false };
    }
  }

  global.AuroraAuditedSave = { run: run };
})(window);
