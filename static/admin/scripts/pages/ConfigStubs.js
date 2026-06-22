// Arc A scaffolding — placeholder pages for routes whose real content
// ships in a later arc. Arc A owns only the route *definitions* (§5.7.2)
// and these stub mounts; the page bodies are owned elsewhere:
//
//   configFederationPolicy     → Arc G (placeholder content, 0.9.0;
//   configRegistrationPolicy        activates per-page as backends land
//   configModerationPolicy          across 0.9.x — §13.4)
//   configKryphocronPolicy
//   configIntegrationHooks
//   configObservability
//   kryphocronStub             → Arc D (Kryphocron domain, 0.9.1)
//
// Each stub renders the §5.5.4 "in development / available when X ships"
// framing rather than 404-ing, so the IA is navigable end-to-end from
// 0.9.0 (per the Arc A verification gate). When the owning arc lands its
// real page, it registers the same key and removes the entry here — the
// router's last-registration-wins means load order would otherwise let a
// stub shadow a real page, so the owning arc MUST delete its row below.
//
// Strings here are deliberately plain English literals, not t()-routed:
// these bodies are throwaway scaffolding and the owning arc authors the
// real (i18n-routed) content. See recon §4.2/§4.3 for the i18n posture.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  // { key, domain, title, blurb } — `domain` drives the breadcrumb prefix.
  const STUBS = [
    // configFederationPolicy retired in #344 — the real page
    // (ConfigFederationPolicy.js) registers that key now; a read-only posture
    // surface over the env-configured federation state + the two public
    // describe endpoints, with runtime-mutable policy reserved for a later cycle.
    // configRegistrationPolicy retired in #342 — the real page
    // (ConfigRegistrationPolicy.js) registers that key now; it consolidates the
    // already-shipped registration settings (read-only overview + edit-elsewhere
    // links), with the novel rate/IP/blocklist controls reserved for a later cycle.
    // configModerationPolicy retired in #340 — the real page
    // (ConfigModerationPolicy.js) registers that key now; it hosts the
    // moderation-tier switch (moved from UI & modes), with configurable
    // defaults as an in-page in-development section.
    // configKryphocronPolicy retired in #227 (D-policy-page) — the real
    // page (ConfigKryphocronPolicy.js) registers that key now.
    {
      key: 'configIntegrationHooks',
      domain: 'Configuration',
      domainRoute: 'configuration/general',
      title: 'Integration hooks',
      blurb: 'Available when the integration-hooks backend ships. This page ' +
             'will manage webhook configuration and external moderation pairing.',
    },
    // configObservability retired in #343 — the real page
    // (ConfigObservability.js) registers that key now; a read-only status/
    // overview surface (Recovery-Mode pattern) over the shipped observability
    // surfaces + env-scoped config docs, with runtime telemetry config reserved
    // for a later cycle.
    // The `kryphocronStub` Overview placeholder was retired in #226 (D-routes):
    // the Kryphocron domain's real routes (overview / laquna / laquna-history /
    // audiences / tier-activity) ship across #228–#231 + #229. The
    // `configKryphocronPolicy` stub below stays until #227 (D-policy-page)
    // registers the real page and deletes its row.
  ];

  function makeMount(stub) {
    return function mount(ctx) {
      const container = (ctx && ctx.container) || null;
      if (!container) return {};
      container.innerHTML =
        '<nav class="breadcrumb" aria-label="Breadcrumb">' +
        '  <a href="#' + esc(stub.domainRoute) + '">' + esc(stub.domain) + '</a>' +
        '  <span class="breadcrumb-sep">›</span>' +
        '  ' + esc(stub.title) +
        '</nav>' +
        '<header class="page-header">' +
        '  <h2>' + esc(stub.title) + '</h2>' +
        '</header>' +
        '<div class="empty-state" role="status">' +
        '  <p>' + esc(stub.blurb) + '</p>' +
        '</div>';
      return {};
    };
  }

  if (global.AuroraRouter) {
    for (const stub of STUBS) {
      global.AuroraRouter.register(stub.key, { mount: makeMount(stub) });
    }
  }
})(window);
