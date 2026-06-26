// Static pins for AccountDetail endpoint routing (#362.1). The page's account
// MUTATIONS were direct global.AuroraClient.post() calls bypassing the
// endpoints.js registry; this routes each through AuroraEndpoints.atproto.* so
// every account mutation is discoverable in one place. The single read-only,
// single-page diagnostic (listRecords "records authored" preview) deliberately
// stays a direct call per the judgment rule (not a mutation, not reused).
//
//   node static/admin/scripts/pages/__tests__/account-detail-endpoint-routing.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS = path.resolve(__dirname, '..', '..');
const page = fs.readFileSync(path.resolve(SCRIPTS, 'pages', 'AccountDetail.js'), 'utf8');
const endpoints = fs.readFileSync(path.resolve(SCRIPTS, 'api', 'endpoints.js'), 'utf8');

const MUTATIONS = [
  'updateAccountPassword',
  'updateAccountEmail',
  'updateAccountHandle',
  'updateAccountSigningKey',
  'enableAccountInvites',
  'disableAccountInvites',
  'deleteAccount',
];

test('endpoints.js registers each account-mutation wrapper as a POST', () => {
  for (const m of MUTATIONS) {
    const re = new RegExp(m + ":\\s*\\(body\\)\\s*=>\\s*C\\(\\)\\.post\\('com\\.atproto\\.admin\\." + m + "'");
    assert.ok(re.test(endpoints), 'endpoints.js wraps ' + m + ' as a POST');
  }
});

test('AccountDetail routes every mutation through AuroraEndpoints.atproto.*', () => {
  for (const m of MUTATIONS) {
    assert.ok(
      page.includes('AuroraEndpoints.atproto.' + m),
      'AccountDetail calls AuroraEndpoints.atproto.' + m,
    );
  }
});

test('AccountDetail no longer posts account mutations via direct AuroraClient', () => {
  // No direct .post() to any of the wrapped admin NSIDs remains.
  for (const m of MUTATIONS) {
    const re = new RegExp("AuroraClient\\.post\\('com\\.atproto\\.admin\\." + m + "'");
    assert.ok(!re.test(page), 'no direct AuroraClient.post for ' + m);
  }
});

test('the getInviteCodes read routes through the existing registry wrapper', () => {
  assert.ok(
    page.includes('AuroraEndpoints.atproto.getInviteCodes('),
    'invite-lineage read uses the registry wrapper',
  );
  assert.ok(
    !/AuroraClient\.get\('com\.atproto\.admin\.getInviteCodes'/.test(page),
    'no direct AuroraClient.get for getInviteCodes',
  );
});

test('the single-page listRecords diagnostic stays a direct call (judgment rule)', () => {
  // It is read-only, single-page, and not reused — it earns no wrapper.
  assert.ok(
    /AuroraClient\.get\('com\.atproto\.repo\.listRecords'/.test(page),
    'listRecords remains a direct diagnostic read',
  );
});
