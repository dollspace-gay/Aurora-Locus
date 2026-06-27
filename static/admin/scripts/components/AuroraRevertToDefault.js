// AuroraRevertToDefault — the "Revert to default" control primitive
// (v0.9 Federation runtime-mutability arc §4.2, #402).
//
// A runtime-setting card whose source tier is `Runtime` (an operator override)
// can be reverted to its environment-seeded default by deleting the runtime row.
// This wraps the confirm-with-rationale flow (AuroraModal.destructiveConfirm,
// audited on delete) + the `deleteRuntimeSetting` XRPC (C4). H1 places the
// button on each editable card and calls `run`.
//
//   AuroraRevertToDefault.run({
//     key,                 // runtime-settings key, e.g. 'federation.enabled'
//     label,               // human label for the dialog, e.g. 'Federation enabled'
//     envVar,              // optional: env var name shown in the dialog
//     isRestartRequired,   // restart-required fields queue a marker on revert (C4)
//   }) → Promise<{ reverted: boolean, auditEntryId?: string, rationale?: string }>
//
// For restart-required fields the deletion ALSO queues the pending_restart
// marker(s) server-side (C4's outer-tx), so on success this refreshes the
// queued-change banner (F3) — the operator then restarts from there.

(function (global) {
  'use strict';

  async function run(spec) {
    spec = spec || {};
    const key = spec.key;
    const labelText = spec.label || key;
    const envVar = spec.envVar || '';
    const restart = !!spec.isRestartRequired;

    // Plain text — AuroraModal.destructiveConfirm escapes the body.
    let bodyText =
      'This deletes the runtime override and restores the value from the environment configuration' +
      (envVar ? ' (' + envVar + ')' : '') + '.';
    if (restart) {
      bodyText += ' ' + labelText +
        ' is a restart-required field — after reverting, the change appears in the' +
        ' pending-restart banner; click "Restart now" there when ready.';
    }

    const res = await global.AuroraModal.destructiveConfirm({
      heading: 'Revert "' + labelText + '" to default?',
      body: bodyText,
      rationaleRequired: true,
      confirmLabel: 'Revert',
    });
    if (!res || !res.confirmed) return { reverted: false };

    const out = await global.AuroraEndpoints.superadmin.deleteRuntimeSetting({
      key: key,
      rationale: res.rationale || '',
    });

    // A restart-required revert set a marker server-side — surface it now.
    if (restart && global.AuroraQueuedChangeBanner && global.AuroraQueuedChangeBanner.refresh) {
      global.AuroraQueuedChangeBanner.refresh();
    }
    return {
      reverted: true,
      auditEntryId: out && out.auditEntryId,
      rationale: res.rationale,
    };
  }

  global.AuroraRevertToDefault = { run: run };
})(window);
