// Regression pin for #298 — tools.aurora.admin.getModerationMetrics 405.
//
// The dashboard metrics block sent getModerationMetrics as POST, but the route
// is registered GET (axum_extra Query per the XRPC `query` convention,
// chainlink #118) → 405 Method Not Allowed → the metrics panel failed to load
// for admin/superadmin. Fix: the wrapper uses GET, and client.get expands array
// params into REPEATED query keys (the handler's `metrics` Vec needs
// `metrics=a&metrics=b`, which a plain `new URLSearchParams(obj)` would
// comma-join into one value → 400).
//
// Static-source pins. No framework —
//   node static/admin/scripts/api/__tests__/metrics-get-verb.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.join(SCRIPTS_DIR, rel), 'utf8');

test('getModerationMetrics is a GET (matches the route registration)', () => {
  const src = read('api/endpoints.js');
  const line = src.split('\n').find((l) => l.includes('getModerationMetrics:'));
  assert.ok(line, 'getModerationMetrics wrapper must exist');
  assert.match(line, /C\(\)\.get\(/, 'getModerationMetrics must use GET (route is registered get())');
  assert.doesNotMatch(line, /C\(\)\.post\(/, 'getModerationMetrics must NOT POST (caused the 405)');
});

test('client.get expands array params into repeated query keys', () => {
  const src = read('api/client.js');
  assert.match(src, /Array\.isArray\(/, 'client.get must special-case array params');
  assert.match(src, /\.append\(/, 'client.get must append (repeated keys), not comma-join');
});
