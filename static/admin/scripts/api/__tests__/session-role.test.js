// Regression pin for #297 — admin UI resolved every operator as "moderator".
//
// Root cause: getSession returned no role, and the dev-login path sets no
// cached `adminRole`, so AuroraSession.role() fell through to its 'moderator'
// last-resort fallback for admin/superadmin too — breaking the sidebar/route
// gating for every tier above moderator. The backend now returns `role` on
// getSession (src/api/server.rs / SessionInfo), and the frontend already reads
// it via AuroraSession.role() → currentUser.role. These pins lock the frontend
// side of that contract: role is read from the live session, with 'moderator'
// only as the final fallback.
//
// Static-source pins. No framework —
//   node static/admin/scripts/api/__tests__/session-role.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.join(SCRIPTS_DIR, rel), 'utf8');

test('AuroraSession.role() resolves currentUser.role before any fallback', () => {
  const src = read('state/session.js');
  assert.match(
    src,
    /currentUser\s*&&\s*currentUser\.role\)\s*return\s+currentUser\.role/,
    'role() must return currentUser.role first (the getSession-provided live role)',
  );
  const idxRole = src.indexOf('currentUser.role');
  const idxFallback = src.lastIndexOf("return 'moderator'");
  assert.ok(
    idxRole !== -1 && idxFallback !== -1 && idxRole < idxFallback,
    "'moderator' must be the last-resort fallback, after currentUser.role",
  );
});

test('app.js bootstrap feeds the getSession response (with role) into the session', () => {
  const src = read('app.js');
  assert.match(src, /getSession\(\)/, 'bootstrap calls getSession');
  assert.match(src, /setUser\(sess\)/, 'bootstrap stores the session (carrying role) via setUser');
});
