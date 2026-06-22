// Configuration → Observability page (route: #configuration/observability).
//
// A read-only status/overview surface (#343, Arc G stub activation; recon
// docs/internal/v09/v09_arc_g_recon.md §G.4). The Recovery-Mode (#276) pattern:
// expose what's true, link to the deeper surfaces that already poll (System
// Health, Dashboard, Audit log), and document the env/deploy-scoped config
// operators tune at startup — with NO fake controls and NO fabricated status
// reads. Sibling to the Registration consolidation page (#342).
//
// Live summary reads (honest, from existing endpoints): overall system-health
// status, database backend + pool, kryphocron audience-oracle consultation
// total. Everything else is documentation + links:
//   - /metrics — the Prometheus endpoint exists and is scraped per the
//     deployment's Prometheus config; there is no admin-side reachability probe,
//     so it's documented, not badged.
//   - Log level — RUST_LOG is a startup env var, not exposed by any endpoint and
//     not runtime-settable; a behaviour note, not a read (the #342 discipline:
//     no status read from a source that can't honestly supply it).
//   - Audit log — append-only/tamper-evident; linked, with no fabricated size.
// Runtime-settable telemetry config is reserved for a later cycle (§7.5 line
// 2124, contracts undesigned) — honest "coming in a future cycle" framing.
//
// Read-only: no setRuntimeSetting, no new XRPCs, no audit emission. A manual
// Refresh re-runs the live reads (the deeper surfaces own the 30s polling; this
// overview doesn't duplicate that load). Per-section reads fail independently.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Observability</nav>' +
      '<header class="page-header"><div><h2>Observability</h2>' +
      '<p class="page-subtitle">Where to observe this deployment, and how telemetry is configured</p></div>' +
      '<button type="button" class="btn-secondary" id="obs-refresh">Refresh</button></header>' +
      '<div class="settings-grid">' +
      '  <div class="settings-card">' +
      '    <h3>Metrics endpoint</h3>' +
      '    <p>Prometheus metrics are exposed at <code>/metrics</code>.</p>' +
      '    <p class="settings-help">Scrape interval and target config live in your Prometheus deployment, not in Aurora-Locus. For live in-app metrics, see System Health and the Dashboard.</p>' +
      '    <p><a href="#ops/system-health">Open Operations → System Health</a></p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>System health</h3>' +
      '    <p>Status: <strong id="obs-health">Loading…</strong></p>' +
      '    <p><a href="#ops/system-health">Open Operations → System Health</a></p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Database</h3>' +
      '    <p id="obs-db">Loading…</p>' +
      '    <p><a href="#ops/system-health">Detail on System Health</a></p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Logging</h3>' +
      '    <p class="settings-help">The log level is set by the <code>RUST_LOG</code> environment variable at startup — a deployment configuration, not a runtime toggle. Change it and restart to adjust verbosity.</p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Audit log</h3>' +
      '    <p class="settings-help">The administrative audit log is append-only and tamper-evident (hash-chained); substrate-action entries are not auto-pruned.</p>' +
      '    <p><a href="#mod/audit">Open Audit log</a></p>' +
      '  </div>' +
      '  <div class="settings-card">' +
      '    <h3>Substrate metrics</h3>' +
      '    <p>Kryphocron audience-oracle consultations: <strong id="obs-oracle">Loading…</strong></p>' +
      '    <p><a href="#kryphocron/overview">Kryphocron Overview</a> · <a href="#dashboard">Dashboard</a></p>' +
      '  </div>' +
      '</div>' +
      // Design-deferred runtime telemetry config (§7.5 line 2124, contracts
      // undesigned). Honest framing per the #340/#342 convention — describe the
      // work, promise no version, ship no fake controls.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Coming in a future cycle</h3>' +
      '  <p class="settings-help">Runtime-settable log levels, telemetry-redaction posture, and log-aggregation endpoint configuration are reserved for a later release once their contracts are designed; they are not configurable yet.</p>' +
      '</section>';

    const btn = document.getElementById('obs-refresh');
    if (btn) btn.addEventListener('click', loadLive);
    await loadLive();
    return {};
  }

  // The honest live reads. Each is isolated — one failing endpoint shows
  // "Unavailable" for its field without blanking the others.
  async function loadLive() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;

    const healthEl = document.getElementById('obs-health');
    try {
      const h = await ep.ops.getSystemHealth();
      if (healthEl) healthEl.textContent = (h && typeof h.status === 'string') ? h.status : 'unknown';
    } catch (e) {
      if (healthEl) healthEl.textContent = 'Unavailable';
    }

    const dbEl = document.getElementById('obs-db');
    try {
      const d = await ep.ops.getDatabaseStatus();
      const backend = (d && d.backend) ? esc(d.backend) : '—';
      const pool = (d && (d.poolUsed != null || d.poolMax != null))
        ? ' · pool ' + esc(d.poolUsed || 0) + ' / ' + esc(d.poolMax || 0) : '';
      if (dbEl) dbEl.innerHTML = '<strong>Backend:</strong> ' + backend + pool;
    } catch (e) {
      if (dbEl) dbEl.textContent = 'Unavailable';
    }

    const oracleEl = document.getElementById('obs-oracle');
    try {
      const o = await ep.ops.kryphocron.getOracleActivity();
      if (oracleEl) {
        oracleEl.textContent = (o && o.instrumented && o.consultations)
          ? String(o.consultations.total != null ? o.consultations.total : 0)
          : 'not instrumented';
      }
    } catch (e) {
      if (oracleEl) oracleEl.textContent = 'Unavailable';
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configObservability', { mount: mount });
})(window);
