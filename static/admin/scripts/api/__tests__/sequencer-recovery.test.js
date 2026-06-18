// Contract pins for the sequencer-recovery surface (Arc H §7.4.2 / #295):
// the four XRPC wrappers, the page's consumption of them, the validate
// operation id, route + sidebar wiring, and i18n-key completeness.
//
// Static source reads (the page pulls in DOM / router / polling that's
// heavyweight to mock). Mirrors repo-rebuild.test.js / repo-repair.test.js.
//
// No framework dependency — runs under bare Node via `node
// static/admin/scripts/api/__tests__/sequencer-recovery.test.js`.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..', '..'); // static/admin
const read = (rel) => fs.readFileSync(path.join(ROOT, rel), 'utf8');

const ENDPOINTS = read('scripts/api/endpoints.js');
const PAGE = read('scripts/pages/SequencerRecovery.js');
const ROUTES = read('scripts/routing/routes.js');
const EN = JSON.parse(read('i18n/en.json'));

const METHODS = [
  ['sequencerRecoveryOptions', 'get'],
  ['runSequencerRecovery', 'post'],
  ['getSequencerRecoveryProgress', 'get'],
  ['cancelSequencerRecovery', 'post'],
];

test('endpoints.js registers the four sequencer-recovery wrappers with correct NSIDs', () => {
  for (const [m, verb] of METHODS) {
    const re = new RegExp(m + ':[\\s\\S]*?C\\(\\)\\.' + verb + "\\('tools\\.aurora\\.superadmin\\." + m + "'");
    assert.match(ENDPOINTS, re, `${m} wrapper (${verb}) missing or wrong NSID`);
  }
});

test('the page consumes all four endpoints', () => {
  for (const [m] of METHODS) {
    assert.match(PAGE, new RegExp('EP\\(\\)\\.' + m + '\\('), `page must call ${m}`);
  }
});

test('the page dispatches the validate operation', () => {
  assert.match(PAGE, /runSequencerRecovery\(\{\s*operation:\s*'validate'\s*\}\)/);
});

test('the page registers the opsSequencerRecovery route handler', () => {
  assert.match(PAGE, /AuroraRouter\.register\('opsSequencerRecovery'/);
});

test('routes.js wires the recovery route (SuperAdmin) + a sidebar entry', () => {
  assert.match(ROUTES, /pattern:\s*'ops\/sequencer\/recovery',\s*page:\s*'opsSequencerRecovery',\s*requires:\s*'superadmin'/);
  assert.match(ROUTES, /label:\s*'Sequencer recovery',\s*route:\s*'ops\/sequencer\/recovery'[^}]*requires:\s*'superadmin'/);
});

test('every seqRecovery.* i18n key the page references exists in en.json', () => {
  const keys = new Set();
  const re = /t\('seqRecovery\.([a-z_]+)'/g;
  let m;
  while ((m = re.exec(PAGE)) !== null) keys.add(m[1]);
  assert.ok(keys.size >= 15, 'expected the page to reference many seqRecovery.* keys');
  const bucket = EN.seqRecovery || {};
  const missing = [...keys].filter((k) => !(k in bucket));
  assert.deepEqual(missing, [], 'en.json is missing seqRecovery.* keys: ' + missing.join(', '));
});
