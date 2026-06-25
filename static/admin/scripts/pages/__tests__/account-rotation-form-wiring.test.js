// Static pins for the AccountDetail signing-key rotation form (B3 / #374 / §4.5,
// §4.6). The contract dropped the bare `signingKey`; the form now sends a
// rationale and, only when the operator-supplied-keys gate is on, an optional
// operatorKeypair. PDS-generation is the default (blank keypair fields).
//
//   node --test static/admin/scripts/pages/__tests__/account-rotation-form-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const src = fs.readFileSync(
  path.resolve(__dirname, '..', 'AccountDetail.js'),
  'utf8',
);

test('no longer sends the removed bare signingKey field', () => {
  assert.ok(!/signingKey:\s*result\.values/.test(src), 'must not POST a bare signingKey');
});

test('reads the operator-supplied-keys gate before offering keypair fields', () => {
  assert.ok(
    src.includes('key_rotation.operator_supplied_keys_enabled'),
    'form gates the operator-keypair fields on the runtime setting',
  );
});

test('builds the new contract: did + optional rationale + gated operatorKeypair', () => {
  assert.ok(src.includes('payload.rationale'), 'sends rationale');
  assert.ok(src.includes('operatorKeypair'), 'sends operatorKeypair when supplied');
  assert.ok(src.includes('publicDidKey') && src.includes('privateKeyHex'),
    'operator keypair carries publicDidKey + privateKeyHex');
});

test('requires both operator key halves or neither', () => {
  // Partial keypair input is rejected client-side rather than sent malformed.
  assert.ok(/pub \|\| priv/.test(src), 'warns when only one of the keypair fields is filled');
});
