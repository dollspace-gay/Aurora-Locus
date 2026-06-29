// Static pins for the Kryphocron Policy page (#334). The page's five cards
// write runtime-setting keys that the substrate registry now backs, so saves
// succeed (previously every save 400'd — "unknown runtime setting key"). These
// guard that the page routes each key through the audited save path and that the
// key names match the registered allowlist.
//
//   node static/admin/scripts/pages/__tests__/kryphocron-policy-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'ConfigKryphocronPolicy.js'), 'utf8');

test('the page declares the five registered policy keys', () => {
  for (const key of [
    'kryphocron.policy.new-account-access',
    'kryphocron.policy.access-delay-days',
    'kryphocron.policy.default-audience-mode',
    'kryphocron.deployment.process-shape',
    'kryphocron.laquna.account-cadence-range',
  ]) {
    assert.ok(src.includes("'" + key + "'"), 'declares ' + key);
  }
});

test('saves route through the audited setRuntimeSetting path', () => {
  assert.ok(src.includes('AuroraAuditedSave'), 'uses the audited save component');
  // The access card bundles its delay-days companion in one audited save.
  assert.ok(src.includes('KEY_ACCESS_DELAY'), 'access save carries the delay-days companion');
});

test('earned access stays disabled (backend-prereq, rejected by the registry)', () => {
  // The registry validator rejects `earned`; the page must not offer it as an
  // enabled choice (it ships as a disabled option). Match the option entry, not
  // the prose comment that also names `earned`.
  const optIdx = src.indexOf("value: 'earned'");
  assert.ok(optIdx !== -1, 'the earned option is declared');
  const around = src.slice(optIdx, optIdx + 120);
  assert.ok(/disabled:\s*true/.test(around), 'earned option is marked disabled');
});
