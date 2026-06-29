// Federation Pattern-1 Phase E (#355) — Audit page federation-management sibling
// filter + the §6.4 source-dropdown `discovery` value. Static pins mirroring the
// integration-hooks filter precedent.
//
//   node static/admin/scripts/pages/__tests__/audit-federation-filter.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const src = fs.readFileSync(path.resolve(__dirname, '..', 'Audit.js'), 'utf8');

test('federation-management is a BOOL filter key + checkbox', () => {
  assert.ok(/BOOL_KEYS = \[[^\]]*'federationManagement'/.test(src), 'federationManagement in BOOL_KEYS');
  assert.ok(/id: 'federationManagement'.*Federation management/.test(src), 'checkbox present with label');
});

test('federation-management is a one-way-clear sibling (clears §5.5.4 filters)', () => {
  assert.ok(/turnedOnFed/.test(src), 'detects federation filter turn-on');
  // Turning on hook OR federation clears source + ruleManagement.
  assert.ok(/turnedOnHook \|\| turnedOnFed/.test(src), 'either management filter clears the §5.5.4 filters');
});

test('federation-management threads to the query params', () => {
  assert.ok(/lastFilters\.federationManagement\) params\.federationManagement = true/.test(src), 'params wired');
});

test('empty-intersection note covers management AND §5.5.4 (and hook AND federation)', () => {
  assert.ok(/hookAndFed/.test(src), 'hook+federation empty-AND case');
  assert.ok(/management filter .* combined \(AND\)/.test(src), 'note explains the AND-empty case');
});

test('§6.4 source dropdown gains the discovery value', () => {
  assert.ok(/value: 'discovery', label: 'Discovery \(auto-accept\)'/.test(src), 'discovery option added');
});
