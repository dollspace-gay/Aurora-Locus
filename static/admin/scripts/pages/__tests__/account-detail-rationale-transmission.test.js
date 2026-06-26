// Static pins for AccountDetail rationale transmission (#362.5). The page
// already COLLECTED an audit rationale for the password override (form field)
// and the email/handle changes (promptRationale), but dropped it on the floor
// — the request bodies carried only {did, password|email|handle}. The handlers
// now accept an optional `rationale` and record it on the audit chain; these
// pins guard that the UI actually transmits the rationale it collects.
//
//   node static/admin/scripts/pages/__tests__/account-detail-rationale-transmission.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS = path.resolve(__dirname, '..', '..');
const page = fs.readFileSync(path.resolve(SCRIPTS, 'pages', 'AccountDetail.js'), 'utf8');

test('password override transmits the form rationale', () => {
  assert.ok(
    /updateAccountPassword\(\{[\s\S]*?rationale: result\.values\.rationale/.test(page),
    'updateAccountPassword body carries rationale: result.values.rationale',
  );
});

test('email update transmits the prompted rationale', () => {
  assert.ok(
    /updateAccountEmail\(\{[\s\S]*?rationale: rationale/.test(page),
    'updateAccountEmail body carries rationale: rationale',
  );
});

test('handle update transmits the prompted rationale', () => {
  assert.ok(
    /updateAccountHandle\(\{[\s\S]*?rationale: rationale/.test(page),
    'updateAccountHandle body carries rationale: rationale',
  );
});
