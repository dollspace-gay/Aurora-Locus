// Regression pins for chainlink #435 — the admin UI password login form wired to
// /admin-oauth/password-login (the Bearer alternative to the OAuth path, which is
// blocked upstream by the proto-blue-oauth DPoP `exp` issue, #434).
//
// Source-assertion tests (the repo's frontend test pattern): read the page
// source and assert its shape, no runtime/jsdom.
//
//   node --test static/admin/scripts/api/__tests__/password-login-form.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.join(SCRIPTS_DIR, rel), 'utf8');

const js = read('../login/login.js');
const html = read('../login.html');

test('login page has a password form, keeping the OAuth form', () => {
  assert.match(html, /id="password-login-form"/, 'password form present');
  assert.match(html, /handlePasswordLogin\(event\)/, 'form calls handlePasswordLogin');
  assert.match(html, /id="admin-login-identifier"/, 'identifier input present');
  assert.match(html, /id="admin-login-password"/, 'password input present');
  assert.match(html, /type="password"/, 'password input is masked');
  assert.match(html, /id="login-form"/, 'OAuth form retained (not replaced)');
});

test('handlePasswordLogin POSTs to /admin-oauth/password-login as JSON', () => {
  assert.match(js, /function handlePasswordLogin/, 'handler defined');
  assert.match(
    js,
    /fetch\(\s*['"]\/admin-oauth\/password-login['"]/,
    'posts to the password-login endpoint'
  );
  assert.match(js, /method:\s*['"]POST['"]/, 'uses POST');
  assert.match(
    js,
    /JSON\.stringify\(\{\s*identifier,\s*password\s*\}\)/,
    'sends identifier + password'
  );
});

test('handlePasswordLogin stows tokens under the canonical localStorage keys', () => {
  assert.match(
    js,
    /setItem\(\s*['"]aurora-admin-token['"]\s*,\s*data\.access_token/,
    'access-token key matches the OAuth callback + api/client.js'
  );
  assert.match(js, /setItem\(\s*['"]aurora-admin-refresh-token['"]/, 'refresh-token key');
  assert.match(js, /setItem\(\s*['"]adminDid['"]\s*,\s*data\.did/, 'did key');
  assert.match(js, /setItem\(\s*['"]adminRole['"]/, 'role key');
  assert.match(
    js,
    /window\.location\.href\s*=\s*['"]\/admin\/index\.html['"]/,
    'redirects into the admin app on success'
  );
});

test('password login surfaces a generic, non-enumerating error', () => {
  // Same message for 401 (bad identifier/password) and 403 (no admin role).
  assert.match(
    js,
    /Login failed\. Check your credentials or admin role\./,
    'generic error text'
  );
  // The specific HTTP status is only console-logged, never shown to the user.
  assert.match(js, /console\.debug\([^)]*response\.status/, 'status logged for debugging only');
  assert.doesNotMatch(
    js,
    /showError\([^)]*40[13]/,
    'must not surface the HTTP code to the user'
  );
});
