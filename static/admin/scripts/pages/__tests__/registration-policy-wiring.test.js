// Static pins for the Registration policy consolidation page (#342). Read-only
// overview: registration mode from describeServer (the real static config, not
// the unbacked general.invite-required key), #334 access policies via
// getRuntimeSetting, edit-elsewhere links, and the future-cycle stub. Guards
// that it never dual-writes a setting and that the stub row is gone.
//
//   node static/admin/scripts/pages/__tests__/registration-policy-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (name) => fs.readFileSync(path.join(PAGES, name), 'utf8');

test('Registration page registers its key and reads the real registration mode', () => {
  const src = read('ConfigRegistrationPolicy.js');
  assert.ok(/register\('configRegistrationPolicy'/.test(src), 'registers configRegistrationPolicy');
  assert.ok(src.includes('describeServer()'), 'reads the real config via describeServer');
  assert.ok(src.includes('inviteCodeRequired'), 'derives mode from inviteCodeRequired');
});

test('it surfaces the #334 access policies read-only with edit-elsewhere links', () => {
  const src = read('ConfigRegistrationPolicy.js');
  assert.ok(src.includes("getRuntimeSetting('kryphocron.policy.new-account-access')"), 'reads new-account-access');
  assert.ok(src.includes("getRuntimeSetting('kryphocron.policy.default-audience-mode')"), 'reads default-audience-mode');
  assert.ok(src.includes('#configuration/kryphocron-policy') || /kryphocron-policy/.test(src), 'links to Kryphocron policy to edit');
  assert.ok(src.includes('#ops/invites'), 'links to Operations → Invites');
});

test('it is read-only — never writes a setting (no dual-write)', () => {
  const src = read('ConfigRegistrationPolicy.js');
  // Match a call (setRuntimeSetting(...)), not the prose in the header comment.
  assert.ok(!/setRuntimeSetting\s*\(/.test(src), 'no setRuntimeSetting calls on the overview page');
});

test('the future-cycle controls are framed honestly (no fake controls, no version promise)', () => {
  const src = read('ConfigRegistrationPolicy.js');
  assert.ok(/future cycle/i.test(src), 'has a future-cycle section');
  assert.ok(/not configurable yet|not yet|reserved/i.test(src), 'frames the deferred controls honestly');
});

test('the describeServer wrapper exists and the stub row is removed', () => {
  const ep = read('../api/endpoints.js');
  assert.ok(/describeServer:\s*\(\)\s*=>/.test(ep), 'endpoints.js has a describeServer wrapper');
  const stubs = read('ConfigStubs.js');
  assert.ok(!/key:\s*'configRegistrationPolicy'/.test(stubs), 'the registration stub row is removed');
});
