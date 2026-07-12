// Regression pins for the admin login form (chainlink #435 + the #436 submit-
// binding / UX-unification fix).
//
// #436 fixed a credential-in-URL leak: the form was default-submitting as GET
// with the password in the query string. The fix is method="POST" (credentials
// in the body even if JS fails) + a robust addEventListener submit handler that
// preventDefault()s, plus a single identifier field shared by both sign-in paths.
//
// Source-assertion tests (the repo's frontend test pattern): read the page source
// and assert its shape, no runtime/jsdom.
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

test('form POSTs (no GET credential-in-URL leak) with autocomplete hints', () => {
  assert.match(html, /<form id="admin-login-form"[^>]*method="POST"/, 'form must be method="POST"');
  // No inline onsubmit — wired via addEventListener so the binding is robust.
  assert.doesNotMatch(html, /onsubmit=/, 'no inline onsubmit (wired in JS)');
  assert.match(html, /id="admin-login-identifier"[\s\S]*?autocomplete="username"/, 'identifier autocomplete');
  assert.match(html, /id="admin-login-password"[\s\S]*?autocomplete="current-password"/, 'password autocomplete');
  assert.match(html, /id="admin-login-password"/, 'password input present');
  assert.match(html, /type="password"/, 'password input is masked');
});

test('a single identifier field serves both sign-in paths', () => {
  // The old OAuth-only "handle" field is gone; one field feeds both.
  assert.doesNotMatch(html, /id="handle"/, 'the separate OAuth handle field is removed');
  assert.doesNotMatch(html, /id="login-form"/, 'the separate OAuth form is removed');
  assert.match(html, /id="oauth-login-btn"[\s\S]*?type="button"|type="button"[\s\S]*?id="oauth-login-btn"/, 'OAuth is a non-submitting button');
});

test('the submit handler is addEventListener-wired and preventDefaults first', () => {
  assert.match(
    js,
    /getElementById\(['"]admin-login-form['"]\)[\s\S]*?addEventListener\(\s*['"]submit['"]\s*,\s*handlePasswordLogin/,
    'form submit wired to handlePasswordLogin via addEventListener'
  );
  assert.match(
    js,
    /getElementById\(['"]oauth-login-btn['"]\)[\s\S]*?addEventListener\(\s*['"]click['"]\s*,\s*handleOAuthLogin/,
    'OAuth button wired to handleOAuthLogin'
  );
  // preventDefault() must be the first statement of the password handler.
  assert.match(
    js,
    /function handlePasswordLogin\(event\)\s*\{\s*event\.preventDefault\(\);/,
    'handlePasswordLogin calls preventDefault first'
  );
});

test('handlePasswordLogin POSTs JSON to the endpoint and stows canonical keys', () => {
  assert.match(js, /fetch\(\s*['"]\/admin-oauth\/password-login['"]/, 'posts to the endpoint');
  assert.match(js, /method:\s*['"]POST['"]/, 'uses POST');
  assert.match(js, /JSON\.stringify\(\{\s*identifier,\s*password\s*\}\)/, 'sends identifier + password');
  assert.match(js, /setItem\(\s*['"]aurora-admin-token['"]\s*,\s*data\.access_token/, 'access-token key');
  assert.match(js, /setItem\(\s*['"]aurora-admin-refresh-token['"]/, 'refresh-token key');
  assert.match(js, /setItem\(\s*['"]adminDid['"]\s*,\s*data\.did/, 'did key');
  assert.match(js, /setItem\(\s*['"]adminRole['"]/, 'role key');
  assert.match(js, /window\.location\.href\s*=\s*['"]\/admin\/index\.html['"]/, 'redirects into the app');
});

test('OAuth handler reads the shared identifier and rejects a blank one', () => {
  assert.match(js, /function handleOAuthLogin\(event\)\s*\{\s*event\.preventDefault\(\);/, 'preventDefault first');
  assert.match(
    js,
    /handleOAuthLogin[\s\S]*?getElementById\(['"]admin-login-identifier['"]\)/,
    'OAuth reads the unified identifier field'
  );
  assert.match(
    js,
    /handleOAuthLogin[\s\S]*?if\s*\(!identifier\)[\s\S]*?showError\(/,
    'blank identifier is rejected with an error'
  );
  // OAuth must NOT read the removed handle field.
  assert.doesNotMatch(js, /getElementById\(['"]handle['"]\)/, 'no reference to the removed handle field');
});

test('password login surfaces a generic, non-enumerating error', () => {
  assert.match(js, /Login failed\. Check your credentials or admin role\./, 'generic error text');
  assert.match(js, /console\.debug\([^)]*response\.status/, 'status logged for debugging only');
  assert.doesNotMatch(js, /showError\([^)]*40[13]/, 'must not surface the HTTP code to the user');
});
