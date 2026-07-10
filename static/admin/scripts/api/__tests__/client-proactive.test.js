// Unit test for the proactive near-expiry session refresh (#442, Phase 4 ·
// Commit 5). The admin session uses one short lifetime for both tokens, so the
// UI must refresh PROACTIVELY on activity to slide it — reactive-on-401 alone
// can't (both tokens expire together).
//
// No framework: bare Node via `node .../client-proactive.test.js`. Stubs a
// minimal browser global, a controllable clock, and a captured `setTimeout` so
// the timer fires on demand rather than after real wall-clock delay.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

// A JWT whose payload carries `exp` (seconds). Header + signature are dummies —
// decodeExpMs only base64url-decodes the payload.
function jwtWithExp(expSeconds) {
  const b64url = (obj) =>
    Buffer.from(JSON.stringify(obj)).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  return `${b64url({ alg: 'HS256' })}.${b64url({ exp: expSeconds })}.sig`;
}

function makeWindow() {
  const storage = new Map();
  return {
    localStorage: {
      getItem: (k) => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => storage.set(k, String(v)),
      removeItem: (k) => storage.delete(k),
    },
    // Event registry so the substrate's activity listeners land somewhere.
    _listeners: {},
    addEventListener(evt, fn) {
      (this._listeners[evt] = this._listeners[evt] || []).push(fn);
    },
  };
}

// Load client.js against a stub window, with a controllable clock and a
// captured setTimeout. Returns { client, fired(), refreshCalls, setNow }.
function loadClient(win, startNow) {
  let now = startNow;
  const captured = { cb: null, delay: null };
  const refreshCalls = [];

  globalThis.localStorage = win.localStorage;
  globalThis.Date = Object.assign(Object.create(Date), { now: () => now });
  globalThis.atob = (b64) => Buffer.from(b64, 'base64').toString('binary');
  globalThis.setTimeout = (cb, delay) => {
    captured.cb = cb;
    captured.delay = delay;
    return 1;
  };
  globalThis.clearTimeout = () => {};
  globalThis.fetch = (url) => {
    refreshCalls.push(url);
    // A never-resolving refresh is fine: the test only asserts it was CALLED.
    return new Promise(() => {});
  };

  const src = fs.readFileSync(path.resolve(__dirname, '..', 'client.js'), 'utf8');
  const fn = new Function('window', src + '\nreturn window.AuroraClient;');
  const client = fn(win);
  return {
    client,
    capturedDelay: () => captured.delay,
    fire: () => captured.cb && captured.cb(),
    refreshCalls,
    setNow: (n) => {
      now = n;
    },
  };
}

test('decodeExpMs reads a JWT exp and rejects opaque/short tokens', () => {
  const win = makeWindow();
  const { client } = loadClient(win, 0);
  assert.equal(client._proactive.decodeExpMs(jwtWithExp(1234)), 1234 * 1000);
  assert.equal(client._proactive.decodeExpMs('opaque-token'), null);
  assert.equal(client._proactive.decodeExpMs(''), null);
  assert.equal(client._proactive.decodeExpMs(null), null);
});

test('schedule arms a timer ~20% before expiry', () => {
  const win = makeWindow();
  // Token expires at t=1_000_000ms; load clock at t=0 → 1000s lifetime.
  win.localStorage.setItem('aurora-admin-token', jwtWithExp(1000));
  const h = loadClient(win, 0);
  h.client.scheduleProactiveRefresh();
  // lead = max(1_000_000 * 0.2, 30_000) = 200_000; fire in 800_000ms.
  assert.equal(h.capturedDelay(), 800000);
});

test('proactive fire refreshes when the operator was active this window', () => {
  const win = makeWindow();
  win.localStorage.setItem('aurora-admin-token', jwtWithExp(1000));
  const h = loadClient(win, 0);
  win.localStorage.setItem('aurora-admin-refresh-token', 'rt-abc');
  h.client.scheduleProactiveRefresh(); // armed at now=0
  h.setNow(500000);
  h.client._proactive.markActivity(); // activity after arming
  h.fire();
  assert.equal(h.refreshCalls.length, 1, 'should proactively refresh');
  assert.match(h.refreshCalls[0], /\/admin-oauth\/refresh$/);
});

test('proactive fire does NOT refresh an idle session', () => {
  const win = makeWindow();
  win.localStorage.setItem('aurora-admin-token', jwtWithExp(1000));
  const h = loadClient(win, 100); // load (and initial activity) at t=100
  win.localStorage.setItem('aurora-admin-refresh-token', 'rt-abc');
  h.setNow(200); // arm strictly after the last activity → idle
  h.client.scheduleProactiveRefresh();
  h.setNow(800000);
  h.fire(); // no markActivity since arming
  assert.equal(h.refreshCalls.length, 0, 'idle session is left to expire');
});

console.log('client-proactive: all tests passed');
