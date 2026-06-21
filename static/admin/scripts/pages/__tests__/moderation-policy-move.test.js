// Static pins for the moderation-tier IA move (#340): the full/reduced/disabled
// switch moved from UI & modes to the Moderation policy page, with the toast
// honesty fix and a disabled-mode typed-confirm. Guards that the switch lives in
// exactly one place and stays wired to the moderation-mode runtime setting.
//
//   node static/admin/scripts/pages/__tests__/moderation-policy-move.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (name) => fs.readFileSync(path.join(PAGES, name), 'utf8');

test('Moderation policy page hosts the tier switch wired to moderation-mode', () => {
  const src = read('ConfigModerationPolicy.js');
  assert.ok(/register\('configModerationPolicy'/.test(src), 'registers configModerationPolicy');
  for (const v of ['full', 'reduced', 'disabled']) {
    assert.ok(src.includes('value="' + v + '"'), 'tier option ' + v);
  }
  assert.ok(src.includes("setRuntimeSetting({ key: 'moderation-mode'"), 'saves moderation-mode');
  assert.ok(src.includes("'moderation-mode-redirect-url'"), 'handles the disabled redirect URL');
  assert.ok(/Configurable moderation defaults/i.test(src), 'carries the in-development defaults section');
});

test('switching to disabled requires a typed-confirm', () => {
  const src = read('ConfigModerationPolicy.js');
  assert.ok(src.includes('typedConfirmGate'), 'disabled switch is typed-confirm gated');
  assert.ok(/value === 'disabled'/.test(src), 'gates specifically on the disabled tier');
});

test('the toast no longer hedges ("may re-render")', () => {
  const src = read('ConfigModerationPolicy.js');
  assert.ok(!/may re-render/i.test(src), 'no stale "may re-render" hedge');
});

test('UI & modes no longer hosts the moderation tier', () => {
  const src = read('ConfigUiModes.js');
  assert.ok(!src.includes('mod-mode'), 'no moderation-mode controls remain on UI & modes');
  assert.ok(!/setRuntimeSetting\([^)]*moderation-mode/.test(src), 'no moderation-mode save on UI & modes');
});

test('ConfigStubs no longer registers a configModerationPolicy stub', () => {
  const src = read('ConfigStubs.js');
  assert.ok(!/key:\s*'configModerationPolicy'/.test(src), 'the stub row is removed (real page owns the key)');
});
