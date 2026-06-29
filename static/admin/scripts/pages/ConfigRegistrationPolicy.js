// Configuration → Registration policy page (route: #configuration/registration-policy).
//
// A read-only consolidation/overview of the deployment's already-shipped
// registration-relevant settings (#342, Arc G stub activation; recon
// docs/internal/v09/v09_arc_g_recon.md §G.2). Per design §5.5.4 / §7.5 line 2121
// this page "consolidates" existing settings and reserves the novel
// rate-limit / per-IP / blocklist controls for a later cycle.
//
// Option B (read-only status + edit-elsewhere links): the canonical edit homes
// already exist and several values aren't runtime-settable, so this page never
// dual-writes a setting — it surfaces status and points at the owning surface:
//   - registration mode (invites.required) — static startup config, read from
//     the real describeServer.inviteCodeRequired (NOT the unbacked
//     general.invite-required key ConfigGeneral reads); deployment-config only.
//   - new-account access + default audience (#334 kryphocron.policy.* runtime
//     keys) — real values via getRuntimeSetting; edited on Kryphocron policy.
//   - invite codes — managed on Operations → Invites.
// Email verification has no accurate admin source (no describeServer field; the
// general.email-verification runtime key is unbacked; it's mailer-gated at
// signup) — surfaced as an honest behaviour note, not a fabricated status read.
//
// Read-only: no setRuntimeSetting calls, no new XRPCs, no audit emission (edits
// happen on the owning surfaces, which audit themselves). Per-section fetches
// fail independently (partial-success).

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  // A read-only status card: title, a <strong> value slot (id), help text, and
  // an optional edit-elsewhere link.
  function statusCard(title, valueId, help, link) {
    return '<div class="settings-card">' +
      '  <h3>' + esc(title) + '</h3>' +
      '  <p>Current: <strong id="' + valueId + '">Loading…</strong></p>' +
      (help ? '  <p class="settings-help">' + help + '</p>' : '') +
      (link ? '  <p><a href="#' + esc(link.route) + '">' + esc(link.label) + '</a></p>' : '') +
      '</div>';
  }

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Registration policy</nav>' +
      '<header class="page-header"><div><h2>Registration policy</h2>' +
      '<p class="page-subtitle">An overview of the deployment\'s account-registration settings</p></div></header>' +
      '<div class="settings-grid">' +
      statusCard(
        'Account registration mode', 'rp-mode',
        'Set at deployment startup (invites.required). Open accepts any registration; invite-only requires a code. Change it in the deployment configuration and restart.',
        null,
      ) +
      statusCard(
        'Handle domains', 'rp-domains',
        'The handle domains new accounts can register under (deployment configuration).',
        null,
      ) +
      '  <div class="settings-card">' +
      '    <h3>Email verification</h3>' +
      '    <p class="settings-help">Verification emails are sent at signup when the deployment has an SMTP mailer configured — a startup configuration, not a runtime toggle.</p>' +
      '  </div>' +
      statusCard(
        'New-account access', 'rp-access',
        'Whether new accounts can post to the private tier immediately or after a delay.',
        { route: 'configuration/kryphocron-policy', label: 'Edit in Kryphocron policy' },
      ) +
      statusCard(
        'Default audience for new accounts', 'rp-audience',
        'The kryphocron audience mode each new account starts with (or none).',
        { route: 'configuration/kryphocron-policy', label: 'Edit in Kryphocron policy' },
      ) +
      '  <div class="settings-card">' +
      '    <h3>Invite codes</h3>' +
      '    <p class="settings-help">Generate, view usage, and disable invite codes.</p>' +
      '    <p><a href="#ops/invites">Open Operations → Invites</a></p>' +
      '  </div>' +
      '</div>' +
      // The design-deferred novel controls (§7.5 line 2121). Honest framing per
      // the #340/#335 convention: describe what a future cycle adds; don't
      // promise a version — the contracts aren't designed yet.
      '<hr class="config-section-divider">' +
      '<section class="installed-themes-section">' +
      '  <h3>Coming in a future cycle</h3>' +
      '  <p class="settings-help">Registration rate-limit policy, per-IP registration limits, and a registration blocklist are reserved for a later release once their policy contracts are designed; they are not configurable yet.</p>' +
      '</section>';

    await Promise.all([loadServerDescriptor(), loadAccessPolicies()]);
    return {};
  }

  // Registration mode + handle domains from the real static config via the
  // public describeServer descriptor.
  async function loadServerDescriptor() {
    const ep = global.AuroraEndpoints;
    const modeEl = document.getElementById('rp-mode');
    const domEl = document.getElementById('rp-domains');
    try {
      const d = await ep.atproto.describeServer();
      if (modeEl) modeEl.textContent = (d && d.inviteCodeRequired) ? 'Invite-only' : 'Open';
      const domains = (d && Array.isArray(d.availableUserDomains)) ? d.availableUserDomains : [];
      if (domEl) domEl.textContent = domains.length ? domains.join(', ') : '—';
    } catch (e) {
      if (modeEl) modeEl.textContent = 'Unavailable';
      if (domEl) domEl.textContent = 'Unavailable';
    }
  }

  // New-account access + default audience from the #334 runtime keys (real
  // values; edited on the Kryphocron policy page).
  async function loadAccessPolicies() {
    const ep = global.AuroraEndpoints;
    const accessEl = document.getElementById('rp-access');
    const audienceEl = document.getElementById('rp-audience');
    try {
      const mode = await ep.admin.getRuntimeSetting('kryphocron.policy.new-account-access');
      const value = (mode && typeof mode.value === 'string') ? mode.value : 'immediate';
      if (value === 'delayed') {
        let days = 7;
        try {
          const d = await ep.admin.getRuntimeSetting('kryphocron.policy.access-delay-days');
          if (d && (typeof d.value === 'number' || typeof d.value === 'string')) days = d.value;
        } catch (e) { /* default 7 */ }
        if (accessEl) accessEl.textContent = 'delayed (' + days + ' day(s))';
      } else if (accessEl) {
        accessEl.textContent = value;
      }
    } catch (e) {
      if (accessEl) accessEl.textContent = 'Unavailable';
    }
    try {
      const aud = await ep.admin.getRuntimeSetting('kryphocron.policy.default-audience-mode');
      if (audienceEl) audienceEl.textContent = (aud && typeof aud.value === 'string') ? aud.value : 'nobody';
    } catch (e) {
      if (audienceEl) audienceEl.textContent = 'Unavailable';
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configRegistrationPolicy', { mount: mount });
})(window);
