// Static pins for the Observability read-only status page (#343). Recovery-Mode
// pattern: honest live summaries + links + env-config docs, NO fake status reads
// (no fabricated /metrics-reachable badge, no fake RUST_LOG level, no fake audit
// size), and read-only (no setRuntimeSetting). Guards the no-fake-status
// discipline the #342 recon-correct established.
//
//   node static/admin/scripts/pages/__tests__/observability-status-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (name) => fs.readFileSync(path.join(PAGES, name), 'utf8');
const src = read('ConfigObservability.js');

test('registers its key and makes the honest live reads', () => {
  assert.ok(/register\('configObservability'/.test(src), 'registers configObservability');
  assert.ok(src.includes('getSystemHealth()'), 'reads system health');
  assert.ok(src.includes('getDatabaseStatus()'), 'reads database status (backend + pool)');
  assert.ok(src.includes('getOracleActivity()'), 'reads the #335 oracle-activity total');
});

test('links to the deeper observability surfaces instead of re-rendering them', () => {
  assert.ok(src.includes('#ops/system-health'), 'links to System Health');
  assert.ok(src.includes('#mod/audit'), 'links to the Audit log');
  assert.ok(/#kryphocron\/overview|#dashboard/.test(src), 'links to a substrate-metrics surface');
});

test('no fake status reads — env/deploy config is documented, not fabricated', () => {
  // /metrics: documented endpoint, no "reachable" probe/badge.
  assert.ok(src.includes('/metrics'), 'documents the metrics endpoint');
  assert.ok(!/reachable/i.test(src), 'no fabricated /metrics reachability badge');
  // Log level: behaviour note, not a read of a level.
  assert.ok(/RUST_LOG/.test(src), 'documents RUST_LOG as the (startup) log-level source');
  assert.ok(!/getRuntimeSetting/.test(src), 'does not read a (non-existent) runtime log-level key');
});

test('read-only and uses a manual refresh, not a poll', () => {
  assert.ok(!/setRuntimeSetting\s*\(/.test(src), 'no setRuntimeSetting calls');
  assert.ok(src.includes('obs-refresh'), 'has a manual refresh affordance');
  assert.ok(!/setInterval/.test(src), 'no auto-poll (the deeper surfaces own polling)');
});

test('future-cycle telemetry config framed honestly; stub row removed', () => {
  assert.ok(/future cycle/i.test(src) && /not configurable yet|reserved/i.test(src), 'honest deferral framing');
  const stubs = read('ConfigStubs.js');
  assert.ok(!/key:\s*'configObservability'/.test(stubs), 'observability stub row removed');
});
