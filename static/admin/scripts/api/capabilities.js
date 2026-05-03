// Capability-routed substrate (substrate primitive 21).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §6.17:
// - UI components calling endpoints don't know which version is
//   available. Substrate detects via tools.aurora.describeCapabilities
//   and routes calls.
// - Capabilities cached in localStorage on session start; refreshed
//   every 60 minutes or on demand.
// - Feature → NSID mapping table; future endpoint transitions add
//   to the table without component changes.
//
// Vanilla JS, no build steps (per §12.9). Exposes:
//   AuroraCapabilities.getCapabilities()  — current capability set
//   AuroraCapabilities.hasCapability(cap) — bool check
//   AuroraCapabilities.callEndpoint(feature, params) — route + call

(function (global) {
  'use strict';

  const CACHE_KEY = 'aurora.capabilities';
  const CACHE_TTL_MS = 60 * 60 * 1000; // 60 minutes per §6.17
  const API_BASE = '/xrpc';

  // Feature → endpoint routing table. Per §6.17:
  //   { capability, primaryNsid, fallbackNsid }
  // When `capability` is present in describeCapabilities response,
  // route to `primaryNsid`; otherwise fall back to `fallbackNsid`.
  // Adding a new feature transition: append a row here, no
  // component changes needed.
  //
  // Capability vocabulary mirrors §8.15 v0.2 list. The `primaryNsid`
  // column for emit-mod-event points to the Phase 3.5 unified
  // surface; the fallback chain is the older per-action endpoints
  // (kept live for protocol compatibility per §9.2).
  const FEATURE_ROUTES = {
    'emit-mod-event': {
      capability: 'mod-events-emit-v1',
      primaryNsid: 'tools.aurora.admin.emitEvent',
      fallbackNsid: null, // per-action endpoints chosen by ActionPanel directly
      method: 'POST',
    },
    'batch-takedown-accounts': {
      capability: 'batch-takedown-v1',
      primaryNsid: 'tools.aurora.admin.batchTakedownAccounts',
      fallbackNsid: null,
      method: 'POST',
    },
    'batch-suspend-accounts': {
      capability: 'batch-takedown-v1',
      primaryNsid: 'tools.aurora.admin.batchSuspendAccounts',
      fallbackNsid: null,
      method: 'POST',
    },
    'batch-restore-accounts': {
      capability: 'batch-takedown-v1',
      primaryNsid: 'tools.aurora.admin.batchRestoreAccounts',
      fallbackNsid: null,
      method: 'POST',
    },
    'batch-takedown-records': {
      capability: 'batch-takedown-v1',
      primaryNsid: 'tools.aurora.admin.batchTakedownRecords',
      fallbackNsid: null,
      method: 'POST',
    },
    'batch-apply-label': {
      capability: 'batch-takedown-v1',
      primaryNsid: 'tools.aurora.admin.batchApplyLabel',
      fallbackNsid: null,
      method: 'POST',
    },
    'batch-remove-label': {
      capability: 'batch-takedown-v1',
      primaryNsid: 'tools.aurora.admin.batchRemoveLabel',
      fallbackNsid: null,
      method: 'POST',
    },
    'trigger-password-reset': {
      capability: 'trigger-password-reset-v1',
      primaryNsid: 'tools.aurora.admin.triggerPasswordReset',
      fallbackNsid: null,
      method: 'POST',
    },
  };

  // Capability strings the v0.2 server may advertise but which we
  // don't have a routing rule for yet (Phase 3.7+). Listed here so
  // hasCapability() can answer correctly even before a feature route
  // is registered.
  const KNOWN_CAPABILITIES = new Set([
    'audit-trail-v1',
    'subject-history-v1',
    'subject-context-v1',
    'batch-takedown-v1',
    'moderator-activity-v1',
    'invite-lineage-v1',
    'instance-metrics-v1',
    'appeals-v1',
    'mod-events-stream-v1',
    'mod-events-emit-v1',
    'moderation-metrics-v1',
    'queue-stats-v1',
    'forensic-export-v1',
    'trigger-password-reset-v1',
    'reporter-context-v1',
    'runtime-settings-v1',
  ]);

  let cachedCapabilities = null; // { strings: Set<string>, fetchedAt: number }

  function loadFromStorage() {
    try {
      const raw = localStorage.getItem(CACHE_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      if (!parsed || !parsed.fetchedAt || !Array.isArray(parsed.strings)) return null;
      if (Date.now() - parsed.fetchedAt > CACHE_TTL_MS) return null;
      return { strings: new Set(parsed.strings), fetchedAt: parsed.fetchedAt };
    } catch (e) {
      return null;
    }
  }

  function saveToStorage(caps) {
    try {
      localStorage.setItem(
        CACHE_KEY,
        JSON.stringify({
          strings: Array.from(caps.strings),
          fetchedAt: caps.fetchedAt,
        }),
      );
    } catch (e) {
      // localStorage may be disabled; fail silently.
    }
  }

  function authHeaders() {
    const token = localStorage.getItem('adminToken');
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers.Authorization = 'Bearer ' + token;
    return headers;
  }

  // Fetch fresh capabilities from the server. Returns a promise that
  // resolves to the cached entry. Rejects on network or auth errors.
  async function refreshCapabilities() {
    const res = await fetch(API_BASE + '/tools.aurora.describeCapabilities', {
      headers: authHeaders(),
    });
    if (!res.ok) {
      throw new Error('describeCapabilities returned HTTP ' + res.status);
    }
    const body = await res.json();
    // Capability strings live in the `extensions` array per the
    // existing handler in src/api/admin.rs (each extension has a
    // `name` field). Defensive: handle both legacy flat `families`
    // representation and the extension list.
    const strings = new Set();
    if (Array.isArray(body.extensions)) {
      for (const ext of body.extensions) {
        if (ext && typeof ext.name === 'string') strings.add(ext.name);
      }
    }
    // For Phase 3.5 the server doesn't yet emit individual capability
    // strings in `extensions`; infer from the families list (any
    // family with a non-empty endpoint list implies its associated
    // capabilities). This is a temporary inference until the server-
    // side capability-string emission lands in a future sub-phase.
    if (body.families && typeof body.families === 'object') {
      const adminFamily = body.families['tools.aurora.admin'] || [];
      if (Array.isArray(adminFamily) && adminFamily.includes('emitEvent')) {
        strings.add('mod-events-emit-v1');
      }
      if (Array.isArray(adminFamily) && adminFamily.some((n) => n.startsWith('batch'))) {
        strings.add('batch-takedown-v1');
      }
      if (Array.isArray(adminFamily) && adminFamily.includes('triggerPasswordReset')) {
        strings.add('trigger-password-reset-v1');
      }
      const moderatorFamily = body.families['tools.aurora.moderator'] || [];
      if (Array.isArray(moderatorFamily)) {
        if (moderatorFamily.includes('queryEvents')) strings.add('moderator-activity-v1');
        if (moderatorFamily.includes('getSubjectHistory')) strings.add('subject-history-v1');
        if (moderatorFamily.includes('getSubjectContext')) strings.add('subject-context-v1');
        if (moderatorFamily.includes('listAppeals')) strings.add('appeals-v1');
      }
      const opsFamily = body.families['tools.aurora.ops'] || [];
      if (Array.isArray(opsFamily) && opsFamily.includes('getInstanceMetrics')) {
        strings.add('instance-metrics-v1');
      }
    }
    cachedCapabilities = { strings, fetchedAt: Date.now() };
    saveToStorage(cachedCapabilities);
    return cachedCapabilities;
  }

  // Get the current capability set. Loads from localStorage if fresh,
  // otherwise fetches. Returns a promise.
  async function getCapabilities() {
    if (cachedCapabilities) return cachedCapabilities;
    const fromStorage = loadFromStorage();
    if (fromStorage) {
      cachedCapabilities = fromStorage;
      return cachedCapabilities;
    }
    return refreshCapabilities();
  }

  // Synchronous capability check. Returns false if cache not yet
  // populated; callers should await getCapabilities() first if they
  // need an authoritative answer.
  function hasCapability(name) {
    if (cachedCapabilities && cachedCapabilities.strings.has(name)) return true;
    return false;
  }

  // Resolve a feature name → endpoint NSID. Returns the primary NSID
  // when the capability is present, the fallback otherwise. Throws
  // if neither is available (no path to the feature).
  function getEndpointForFeature(feature) {
    const route = FEATURE_ROUTES[feature];
    if (!route) {
      throw new Error('Unknown feature: ' + feature);
    }
    if (hasCapability(route.capability)) return route.primaryNsid;
    if (route.fallbackNsid) return route.fallbackNsid;
    return null; // Component must use per-action endpoint chosen at the page level.
  }

  // High-level call helper. Routes through capability detection,
  // serializes JSON params, attaches auth headers, parses response.
  // Returns the parsed JSON body on success; throws on HTTP error
  // with the response body message attached when available.
  async function callEndpoint(feature, params) {
    // Ensure capability cache is warm before routing.
    await getCapabilities();
    const route = FEATURE_ROUTES[feature];
    if (!route) throw new Error('Unknown feature: ' + feature);
    const nsid = getEndpointForFeature(feature);
    if (!nsid) {
      throw new Error('No endpoint available for feature: ' + feature);
    }
    const url = API_BASE + '/' + nsid;
    const init = {
      method: route.method || 'POST',
      headers: authHeaders(),
    };
    if (init.method !== 'GET' && params != null) {
      init.body = JSON.stringify(params);
    }
    const res = await fetch(url, init);
    if (!res.ok) {
      let detail = '';
      try {
        const body = await res.json();
        detail = body && (body.message || body.error) ? ': ' + (body.message || body.error) : '';
      } catch (e) {
        // ignore
      }
      const err = new Error('HTTP ' + res.status + detail);
      err.status = res.status;
      throw err;
    }
    if (res.status === 204) return null;
    return res.json();
  }

  global.AuroraCapabilities = {
    getCapabilities: getCapabilities,
    refreshCapabilities: refreshCapabilities,
    hasCapability: hasCapability,
    getEndpointForFeature: getEndpointForFeature,
    callEndpoint: callEndpoint,
    // Exposed for tests / inspection only:
    _knownCapabilities: KNOWN_CAPABILITIES,
    _featureRoutes: FEATURE_ROUTES,
  };
})(window);
