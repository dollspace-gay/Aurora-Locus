// Regression pin for #306 — theme switch left colors stale until reload.
//
// themeHref('default') returned the bare no-id URL (/theme/active.css) so the
// server could resolve the deployment-default; but when the default CHANGED the
// URL didn't, so the browser served the cached stylesheet (typography reflowed
// live via data-theme, colors didn't). The 'default' branch now adds a ?v
// cache-bust keyed on the resolved theme — URL changes when the default does,
// no ?id pinned (server still resolves the default).
//
//   node static/admin/scripts/api/__tests__/theme-cache-bust.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const src = fs.readFileSync(
  path.resolve(__dirname, '..', '..', 'state', 'settings.js'),
  'utf8',
);

test('themeHref default branch cache-busts (?v) keyed on the resolved theme', () => {
  const i = src.indexOf('function themeHref');
  assert.ok(i !== -1, 'themeHref must exist');
  const fn = src.slice(i, i + 500);
  assert.match(fn, /\?v=/, "the 'default' branch must add a ?v cache-bust (not the bare cached URL)");
  assert.match(fn, /resolvedThemeId\(\)/, 'the cache-bust must key on the resolved theme id');
  assert.match(fn, /\?id=/, 'an explicit pref is still pinned via ?id');
});
