// Field-name contract pin for tools.aurora.superadmin.{grantRole, revokeRole}.
//
// The server's GrantRoleRequest / RevokeRoleRequest in
// src/api/admin.rs deserialize from `did`, not `subject`. The admin
// UI previously sent `subject` at three call sites (ConfigRoles.js,
// ConfigRolesMembers.js x2), which caused every grant/revoke to
// fail with a serde deserialization error before reaching the
// handler. This test pins the corrected field name so a future
// rename / regression doesn't silently reintroduce the same bug.
//
// The test is a static read of the page sources rather than a
// running-page test because the pages bring in DOM, AuroraRouter,
// and Modal infrastructure that's heavyweight to mock. The contract
// being pinned is purely the literal property name in the request
// body, which a regex check against the source is the right
// granularity for.
//
// No framework dependency — runs under bare Node via `node
// static/admin/scripts/api/__tests__/endpoints.test.js`.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES_DIR = path.resolve(__dirname, '..', '..', 'pages');

function readPage(name) {
  return fs.readFileSync(path.join(PAGES_DIR, name), 'utf8');
}

// Match a grantRole or revokeRole call together with its body
// literal. The body is the {...} expression immediately following
// the open paren. Captures the body literal so subsequent assertions
// can grep inside it.
const CALL_PATTERN =
  /AuroraEndpoints\.superadmin\.(grantRole|revokeRole)\(\s*(\{[^}]*\})/g;

function findCalls(src) {
  const calls = [];
  let m;
  while ((m = CALL_PATTERN.exec(src)) !== null) {
    calls.push({ method: m[1], body: m[2] });
  }
  return calls;
}

test('ConfigRoles.js grantRole sends did, not subject', () => {
  const src = readPage('ConfigRoles.js');
  const calls = findCalls(src);
  const grants = calls.filter((c) => c.method === 'grantRole');
  assert.equal(grants.length, 1, 'expected exactly one grantRole call site');
  for (const c of grants) {
    assert.match(c.body, /\bdid\s*:/, `grantRole body must include did:; got ${c.body}`);
    assert.doesNotMatch(
      c.body,
      /\bsubject\s*:/,
      `grantRole body must NOT include subject:; got ${c.body}`,
    );
  }
});

test('ConfigRolesMembers.js grantRole sends did, not subject', () => {
  const src = readPage('ConfigRolesMembers.js');
  const grants = findCalls(src).filter((c) => c.method === 'grantRole');
  assert.equal(grants.length, 1, 'expected exactly one grantRole call site');
  for (const c of grants) {
    assert.match(c.body, /\bdid\s*:/);
    assert.doesNotMatch(c.body, /\bsubject\s*:/);
  }
});

test('ConfigRolesMembers.js revokeRole sends did, not subject', () => {
  const src = readPage('ConfigRolesMembers.js');
  const revokes = findCalls(src).filter((c) => c.method === 'revokeRole');
  assert.equal(revokes.length, 1, 'expected exactly one revokeRole call site');
  for (const c of revokes) {
    assert.match(c.body, /\bdid\s*:/);
    assert.doesNotMatch(c.body, /\bsubject\s*:/);
  }
});

test('no other admin pages call grantRole or revokeRole with the old subject field', () => {
  // Defensive sweep — if a future page picks up the role-management
  // pattern, it picks up the correct field name. Walks every .js
  // file under pages/ and asserts no `subject:` appears inside a
  // body literal handed to grantRole/revokeRole.
  const files = fs.readdirSync(PAGES_DIR).filter((f) => f.endsWith('.js'));
  for (const f of files) {
    const src = fs.readFileSync(path.join(PAGES_DIR, f), 'utf8');
    for (const c of findCalls(src)) {
      assert.doesNotMatch(
        c.body,
        /\bsubject\s*:/,
        `${f}: ${c.method} body must NOT include subject:; got ${c.body}`,
      );
    }
  }
});
