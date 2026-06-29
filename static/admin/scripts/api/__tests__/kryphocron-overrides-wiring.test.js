// Static pins for per-account kryphocron overrides (#316): the endpoint
// wrappers' NSIDs, the SuperAdmin Account-Detail drawer wiring, and the i18n
// keys. Guards the wrapper↔substrate contract-drift class (the kind Phase B
// found five of) for this surface.
//
//   node static/admin/scripts/api/__tests__/kryphocron-overrides-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.join(SCRIPTS, rel), 'utf8');

test('endpoints.js wires both override XRPCs to their ops.kryphocron NSIDs', () => {
  const src = read('api/endpoints.js');
  assert.ok(src.includes('getAccountOverrides:'), 'getAccountOverrides wrapper');
  assert.ok(src.includes('tools.aurora.ops.kryphocron.getAccountOverrides'), 'getAccountOverrides NSID');
  assert.ok(src.includes('setAccountOverride:'), 'setAccountOverride wrapper');
  assert.ok(src.includes('tools.aurora.ops.kryphocron.setAccountOverride'), 'setAccountOverride NSID');
});

test('AccountDetail renders a SuperAdmin overrides drawer with an audited save', () => {
  const src = read('pages/AccountDetail.js');
  assert.ok(src.includes('loadOverridesDrawer'), 'override drawer loader');
  assert.ok(src.includes("hasRole('superadmin')"), 'SuperAdmin gate');
  assert.ok(src.includes('setAccountOverride('), 'calls setAccountOverride');
  assert.ok(src.includes('rationale_required'), 'rationale required before save');
});

test('en.json carries the kryphocron.overrides keys the drawer reads', () => {
  const i18n = JSON.parse(fs.readFileSync(path.resolve(SCRIPTS, '..', 'i18n', 'en.json'), 'utf8'));
  const ov = i18n.kryphocron && i18n.kryphocron.overrides;
  assert.ok(ov, 'kryphocron.overrides section');
  for (const k of ['title', 'block_label', 'ratelimit_label', 'ratelimit_note', 'rationale_required', 'audit_pivot', 'saved']) {
    assert.ok(typeof ov[k] === 'string' && ov[k].length, 'key ' + k);
  }
});
