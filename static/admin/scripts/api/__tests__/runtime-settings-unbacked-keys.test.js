// Regression pin for #299 — pages must not present a save affordance for
// runtime-setting keys the substrate registry doesn't back.
//
// Recon found the substrate runtime-settings registry backs these keys
// (moderation-mode, moderation-mode-redirect-url, theme.deployment-default,
// kryphocron.laquna.rotation-cadence, and the two branding.login-* URLs added
// in #328) and nothing consumes the `general.*` keys or
// kryphocron.laquna.account-cadence-range. So ConfigGeneral's 7 keys
// and Laquna's range field can't be saved — they're now read-only until the
// backend wires them (deferred feature work). These pins guard that the
// unbacked write paths stay disabled.
//
// Static-source pins. No framework —
//   node static/admin/scripts/api/__tests__/runtime-settings-unbacked-keys.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES_DIR = path.resolve(__dirname, '..', '..', 'pages');
const read = (name) => fs.readFileSync(path.join(PAGES_DIR, name), 'utf8');

test('ConfigGeneral does not call setRuntimeSetting (page is read-only; keys unbacked)', () => {
  const src = read('ConfigGeneral.js');
  assert.doesNotMatch(
    src,
    /setRuntimeSetting/,
    'ConfigGeneral must not present a save path — its general.* keys are not registry-backed',
  );
});

test('Laquna saveCadence persists rotation-cadence only, not the unbacked range key', () => {
  const src = read('Laquna.js');
  // The cadence save must not include KEY_RANGE (account-cadence-range) in its
  // settings — only KEY_CADENCE (rotation-cadence) is registry-backed.
  const saveStart = src.indexOf('function saveCadence');
  assert.ok(saveStart !== -1, 'saveCadence must exist');
  const saveBody = src.slice(saveStart, saveStart + 700);
  assert.match(saveBody, /key:\s*KEY_CADENCE/, 'saveCadence must persist rotation-cadence');
  // Match the settings-entry form (`key: KEY_RANGE`), not a prose mention of
  // the const in a comment.
  assert.doesNotMatch(saveBody, /key:\s*KEY_RANGE/, 'saveCadence must NOT persist the unbacked account-cadence-range');
});
