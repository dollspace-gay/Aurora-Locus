// Static pins for the SuperAdmin bulk session-revoke affordance (#338): the
// Sessions page's "revoke all for this operator" path, its endpoint wrapper,
// and i18n. Guards the wrapper↔substrate contract and the SuperAdmin/scoped
// gating that keeps the affordance off the self-service view.
//
//   node static/admin/scripts/pages/__tests__/sessions-bulk-revoke-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.join(SCRIPTS, rel), 'utf8');

test('endpoints.js wires revokeOperatorSessions to its NSID', () => {
  const src = read('api/endpoints.js');
  assert.ok(src.includes('revokeOperatorSessions:'), 'wrapper present');
  assert.ok(
    src.includes('tools.aurora.admin.revokeOperatorSessions'),
    'wrapper points at the bulk-revoke NSID',
  );
});

test('Sessions page gates the bulk affordance to SuperAdmin scoped to one operator', () => {
  const src = read('pages/Sessions.js');
  assert.ok(src.includes('renderBulkBar'), 'renders a bulk bar');
  // Only shown for SuperAdmin AND when filtered to one operator with sessions.
  assert.ok(/isSuperadmin\(\)\s*&&\s*filterDid|!isSuperadmin\(\)\s*\|\|\s*!filterDid/.test(src),
    'bulk bar gated on SuperAdmin + a did filter');
  assert.ok(src.includes('revokeOperatorSessions'), 'calls the bulk endpoint');
});

test('bulk revoke uses typed-confirm + rationale and handles self-revoke', () => {
  const src = read('pages/Sessions.js');
  assert.ok(src.includes('typedConfirmGate'), 'typed-confirm gate on the destructive action');
  assert.ok(src.includes('rationaleRequired: true'), 'rationale required');
  // Self bulk-revoke logs the caller out.
  assert.ok(/isSelf[\s\S]{0,400}logout\(\)/.test(src), 'self bulk-revoke bounces to login');
});

test('en.json carries the bulk-revoke keys the page reads', () => {
  const i18n = JSON.parse(fs.readFileSync(path.resolve(SCRIPTS, '..', 'i18n', 'en.json'), 'utf8'));
  const s = i18n.sessions || {};
  for (const k of [
    'bulk_revoke',
    'bulk_revoke_heading',
    'bulk_revoke_body',
    'bulk_revoke_self_body',
    'bulk_revoke_confirm',
    'bulk_revoke_success',
    'bulk_revoke_failed',
  ]) {
    assert.ok(typeof s[k] === 'string' && s[k].length, 'key ' + k);
  }
});
