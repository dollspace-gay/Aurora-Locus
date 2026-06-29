// QueuedChangeBanner — the pending-restart banner primitive
// (v0.9 Federation runtime-mutability arc §4.4, #403).
//
// When a restart-required federation field has been saved (or reverted) but the
// PDS hasn't restarted to apply it, a persistent banner tells the operator what
// is pending and offers "Restart now". Reads `listPendingRestartActions` (C7);
// "Restart now" calls `triggerRestart` (D1). Mounted in the admin shell so it
// shows on every admin page (a queued change activates on the NEXT restart for
// ANY reason, so the operator should see it regardless of which page they're on).
//
//   AuroraQueuedChangeBanner.load(containerEl)  // mount + initial fetch
//   AuroraQueuedChangeBanner.refresh()          // re-fetch (after a save/revert)

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  // Internal marker action → human label + the runtime key whose value to show.
  const KNOWN = {
    'restart-required-for-federation-enabled': { label: 'Federation enabled', key: 'federation.enabled' },
    'restart-required-for-service-public-url': { label: 'Public URL', key: 'service.public_url' },
  };

  let rootEl = null;

  async function load(container) {
    if (container) rootEl = container;
    await refresh();
  }

  async function refresh() {
    if (!rootEl) return;
    let data;
    try {
      data = await global.AuroraEndpoints.superadmin.listPendingRestartActions();
    } catch (e) {
      // Non-SuperAdmin (403) or unavailable → no banner.
      rootEl.innerHTML = '';
      return;
    }
    const actions = (data && data.pendingActions) || [];
    // Operator-facing restart-required changes only — the internal
    // `bulk-diddoc-update` marker's effect is covered by the public-url item and
    // the bulk result surface, so it isn't a separate banner line.
    const shown = actions.filter(function (a) { return a && a.action !== 'bulk-diddoc-update'; });
    if (shown.length === 0) {
      rootEl.innerHTML = '';
      return;
    }

    // Best-effort: resolve the to-be-applied value for the known keys.
    const items = await Promise.all(shown.map(async function (a) {
      const known = KNOWN[a.action];
      let valueText = '';
      if (known) {
        try {
          const rs = await global.AuroraEndpoints.admin.getRuntimeSetting(known.key);
          if (rs && rs.value != null) {
            valueText = typeof rs.value === 'object' ? JSON.stringify(rs.value) : String(rs.value);
          }
        } catch (e) { /* value is best-effort */ }
      }
      return {
        label: known ? known.label : a.action,
        value: valueText,
        createdAt: a.createdAt || '',
      };
    }));

    const lis = items.map(function (it) {
      return '<li><strong>' + esc(it.label) + (it.value ? ': ' + esc(it.value) : '') + '</strong>' +
        (it.createdAt ? ' <span class="page-subtitle">(saved ' + esc(it.createdAt) + ')</span>' : '') +
        '</li>';
    }).join('');

    rootEl.innerHTML =
      '<div class="restart-banner" role="status">' +
      '  <p><strong>Restart-required changes pending</strong></p>' +
      '  <p>The following changes have been saved but will not take effect until the PDS restarts:</p>' +
      '  <ul>' + lis + '</ul>' +
      '  <button type="button" class="btn-primary" id="restart-banner-now">Restart now</button>' +
      '  <p class="page-subtitle">The changes also apply automatically on the next supervisor-driven restart or reboot.</p>' +
      '</div>';
    const btn = rootEl.querySelector('#restart-banner-now');
    if (btn) btn.addEventListener('click', onRestartNow);
  }

  async function onRestartNow() {
    const res = await global.AuroraModal.destructiveConfirm({
      heading: 'Restart the PDS now?',
      body: 'This applies all pending restart-required changes. Expect ~10–20 seconds of unavailability while the substrate restarts under your supervisor.',
      rationaleRequired: true,
      confirmLabel: 'Restart now',
    });
    if (!res || !res.confirmed) return;
    try {
      await global.AuroraEndpoints.superadmin.triggerRestart({ rationale: res.rationale || '' });
    } catch (e) {
      // The process is exiting as part of the restart; the request may not
      // return cleanly. That's expected — the supervisor relaunches.
    }
    if (rootEl) {
      rootEl.innerHTML =
        '<div class="restart-banner" role="status"><p>Restarting… refresh the page in ~20 seconds.</p></div>';
    }
  }

  global.AuroraQueuedChangeBanner = { load: load, refresh: refresh };
})(window);
