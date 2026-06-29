// Layer 1 unit test for capability-routed substrate (§13.2 + §6.17).
//
// Per design doc §13.2:
//   "Capability routing (substrate primitive 21) — given a capability
//    set and a feature request, returns the correct endpoint path.
//    Test cases: capability present, capability absent, multiple
//    capabilities, capability set empty."
//
// No framework dependency — runs under bare Node via `node
// static/admin/scripts/api/__tests__/capabilities.test.js`. Stubs
// `window`, `localStorage`, and `fetch` minimally so the substrate's
// `(function(global) {})(window)` wrapper has somewhere to land.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

// ---------- Stub a minimal browser-ish global ----------
//
// The substrate calls bare `localStorage` and `fetch` from nested
// helpers (browser convention — they're globals on `window`). To
// satisfy that under Node, install the stubs on `globalThis` so the
// bare identifiers resolve. Each `loadSubstrate` call resets
// globalThis to a fresh stub set.
function makeWindow() {
  const storage = new Map();
  const localStorage = {
    getItem: (k) => (storage.has(k) ? storage.get(k) : null),
    setItem: (k, v) => storage.set(k, String(v)),
    removeItem: (k) => storage.delete(k),
    clear: () => storage.clear(),
  };
  return {
    localStorage: localStorage,
    fetch: () => Promise.reject(new Error('fetch not stubbed for this test')),
    // #360 — capabilities.js routes auth headers through AuroraClient (which
    // client.js loads first in the browser). Faithful stub of the canonical
    // builder: merge extras + Bearer from the localStorage fallback path.
    AuroraClient: {
      authHeaders: (extra) => {
        const headers = Object.assign({}, extra || {});
        const token = localStorage.getItem('aurora-admin-token');
        if (token) headers.Authorization = 'Bearer ' + token;
        return headers;
      },
    },
  };
}

function loadSubstrate(stubWindow) {
  // Install stubs on globalThis so bare `localStorage`/`fetch`
  // references inside nested helpers resolve. Each test that needs
  // a fresh substrate calls loadSubstrate again, replacing globals.
  globalThis.localStorage = stubWindow.localStorage;
  globalThis.fetch = stubWindow.fetch;
  // Re-evaluate the source. The substrate attaches its API to the
  // supplied `window` object via `(function(global){...})(window)`.
  const src = fs.readFileSync(
    path.resolve(__dirname, '..', 'capabilities.js'),
    'utf8',
  );
  const fn = new Function('window', src + '\nreturn window.AuroraCapabilities;');
  return fn(stubWindow);
}

test('hasCapability returns false before cache is populated', () => {
  const w = makeWindow();
  const caps = loadSubstrate(w);
  assert.equal(caps.hasCapability('mod-events-emit-v1'), false);
});

test('getEndpointForFeature returns primaryNsid when capability present', async () => {
  const w = makeWindow();
  // Pre-seed the cache with the v0.2 capability vocabulary.
  w.localStorage.setItem(
    'aurora.capabilities',
    JSON.stringify({
      strings: ['mod-events-emit-v1', 'batch-takedown-v1'],
      fetchedAt: Date.now(),
    }),
  );
  const caps = loadSubstrate(w);
  await caps.getCapabilities();
  assert.equal(
    caps.getEndpointForFeature('emit-mod-event'),
    'tools.aurora.admin.emitEvent',
  );
  assert.equal(
    caps.getEndpointForFeature('batch-takedown-accounts'),
    'tools.aurora.admin.batchTakedownAccounts',
  );
});

test('getEndpointForFeature returns null when capability absent', async () => {
  const w = makeWindow();
  w.localStorage.setItem(
    'aurora.capabilities',
    JSON.stringify({ strings: [], fetchedAt: Date.now() }),
  );
  const caps = loadSubstrate(w);
  await caps.getCapabilities();
  // No capability → no fallback → null (caller falls back to per-action endpoint)
  assert.equal(caps.getEndpointForFeature('emit-mod-event'), null);
});

test('getEndpointForFeature throws on unknown feature', async () => {
  const w = makeWindow();
  w.localStorage.setItem(
    'aurora.capabilities',
    JSON.stringify({ strings: [], fetchedAt: Date.now() }),
  );
  const caps = loadSubstrate(w);
  await caps.getCapabilities();
  assert.throws(() => caps.getEndpointForFeature('not-a-real-feature'), /Unknown feature/);
});

test('cache TTL: stale cache (older than 60min) is discarded on load', async () => {
  const w = makeWindow();
  // Seed a cache from "61 minutes ago" so loadFromStorage rejects it.
  w.localStorage.setItem(
    'aurora.capabilities',
    JSON.stringify({
      strings: ['mod-events-emit-v1'],
      fetchedAt: Date.now() - 61 * 60 * 1000,
    }),
  );
  // Stub fetch so refresh doesn't hit network — return empty caps.
  w.fetch = () =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: () => Promise.resolve({ extensions: [], families: {} }),
    });
  const caps = loadSubstrate(w);
  // Force a refresh path; capability shouldn't be present after refresh
  // because the empty server response yields an empty cap set.
  await caps.getCapabilities();
  assert.equal(caps.hasCapability('mod-events-emit-v1'), false);
});

test('refreshCapabilities infers caps from families list', async () => {
  const w = makeWindow();
  w.fetch = () =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: () =>
        Promise.resolve({
          extensions: [],
          families: {
            'tools.aurora.admin': [
              'emitEvent',
              'batchTakedownAccounts',
              'triggerPasswordReset',
            ],
            'tools.aurora.moderator': [
              'queryEvents',
              'getSubjectHistory',
              'getSubjectContext',
              'listAppeals',
            ],
            'tools.aurora.ops': ['getInstanceMetrics'],
          },
        }),
    });
  const caps = loadSubstrate(w);
  await caps.refreshCapabilities();
  assert.ok(caps.hasCapability('mod-events-emit-v1'));
  assert.ok(caps.hasCapability('batch-takedown-v1'));
  assert.ok(caps.hasCapability('trigger-password-reset-v1'));
  assert.ok(caps.hasCapability('moderator-activity-v1'));
  assert.ok(caps.hasCapability('subject-history-v1'));
  assert.ok(caps.hasCapability('subject-context-v1'));
  assert.ok(caps.hasCapability('appeals-v1'));
  assert.ok(caps.hasCapability('instance-metrics-v1'));
});

test('callEndpoint sends POST with auth headers and JSON body', async () => {
  const w = makeWindow();
  w.localStorage.setItem('aurora-admin-token', 'test-token-xyz');
  w.localStorage.setItem(
    'aurora.capabilities',
    JSON.stringify({
      strings: ['mod-events-emit-v1'],
      fetchedAt: Date.now(),
    }),
  );
  let capturedUrl = null;
  let capturedInit = null;
  w.fetch = (url, init) => {
    capturedUrl = url;
    capturedInit = init;
    return Promise.resolve({
      ok: true,
      status: 200,
      json: () => Promise.resolve({ eventId: '42' }),
    });
  };
  const caps = loadSubstrate(w);
  const result = await caps.callEndpoint('emit-mod-event', {
    action: { kind: 'TakedownAccount' },
    rationale: 'spam',
  });
  assert.equal(result.eventId, '42');
  assert.equal(capturedUrl, '/xrpc/tools.aurora.admin.emitEvent');
  assert.equal(capturedInit.method, 'POST');
  assert.equal(capturedInit.headers.Authorization, 'Bearer test-token-xyz');
  assert.equal(capturedInit.headers['Content-Type'], 'application/json');
  const body = JSON.parse(capturedInit.body);
  assert.equal(body.action.kind, 'TakedownAccount');
});

test('callEndpoint throws with status code on HTTP error', async () => {
  const w = makeWindow();
  w.localStorage.setItem(
    'aurora.capabilities',
    JSON.stringify({
      strings: ['mod-events-emit-v1'],
      fetchedAt: Date.now(),
    }),
  );
  w.fetch = () =>
    Promise.resolve({
      ok: false,
      status: 403,
      json: () => Promise.resolve({ error: 'PermissionDenied', message: 'role too low' }),
    });
  const caps = loadSubstrate(w);
  await assert.rejects(
    () => caps.callEndpoint('emit-mod-event', { rationale: 'x' }),
    (err) => err.status === 403 && /role too low/.test(err.message),
  );
});
