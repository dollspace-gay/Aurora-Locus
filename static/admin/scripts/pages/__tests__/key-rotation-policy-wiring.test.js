// Static pins for the Key rotation policy page (B2 / #373 / §4.6). The page
// toggles the operator-supplied-keys feature gate via the audited save path.
// These guard that it declares the registered runtime key, sends a real JSON
// bool (the backend validator rejects "true"/1), routes through AuroraAuditedSave
// (so the flip is confirmed + audit-chained), is SuperAdmin-gated, and that the
// route + sidebar entry are wired.
//
//   node static/admin/scripts/pages/__tests__/key-rotation-policy-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'ConfigKeyRotationPolicy.js'), 'utf8');
const routes = fs.readFileSync(
  path.resolve(__dirname, '..', '..', 'routing', 'routes.js'),
  'utf8',
);
const en = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, '..', '..', '..', 'i18n', 'en.json'), 'utf8'),
);

test('declares the registered operator-supplied-keys runtime key', () => {
  assert.ok(
    src.includes("'key_rotation.operator_supplied_keys_enabled'"),
    'declares the gate key',
  );
});

test('sends a real boolean value, not a string or number', () => {
  // The backend validator (validate_runtime_value) rejects non-bool. The save
  // must pass `value: enabled` (a JS bool), never "true"/"false"/1.
  assert.ok(src.includes('value: enabled'), 'save passes a boolean value');
  assert.ok(!/value:\s*['"]true['"]/.test(src), 'never sends the string "true"');
});

test('routes the flip through the audited save path', () => {
  assert.ok(src.includes('AuroraAuditedSave'), 'uses the audited save component');
  // Distinct enable vs disable confirm copy (flip-on is the meaningful gate).
  assert.ok(src.includes('enable_heading') && src.includes('disable_heading'),
    'distinct enable/disable confirm headings');
});

test('is SuperAdmin-gated in the page body', () => {
  assert.ok(src.includes("hasRole('superadmin')"), 'page guards on superadmin');
});

test('route + sidebar entry are wired SuperAdmin-only', () => {
  assert.ok(
    routes.includes("pattern: 'configuration/key-rotation-policy'"),
    'route pattern registered',
  );
  assert.ok(
    /key-rotation-policy'.*requires:\s*'superadmin'/.test(routes),
    'route requires superadmin',
  );
  assert.ok(
    routes.includes("label: 'Key rotation policy'"),
    'sidebar entry present',
  );
});

test('registers under the configKeyRotationPolicy page id', () => {
  assert.ok(
    src.includes("register('configKeyRotationPolicy'"),
    'registers the page id the route maps to',
  );
});

test('i18n keys the page reads exist', () => {
  for (const k of [
    'title', 'subtitle', 'operator_supplied_title', 'operator_supplied_help',
    'operator_supplied_label', 'enable_heading', 'enable_body',
    'disable_heading', 'disable_body', 'save_success',
  ]) {
    assert.ok(
      en.keyRotation && en.keyRotation.policy && en.keyRotation.policy[k],
      'en.json has keyRotation.policy.' + k,
    );
  }
});
