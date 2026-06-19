// Regression pin for #307 — OS prefers-reduced-motion must be honored.
//
// tokens.css (always-loaded framework bundle) carries a universal
// prefers-reduced-motion rule collapsing every animation/transition to instant
// and disabling smooth-scroll. (Recon: the rule already existed framework-wide;
// #307 added scroll-behavior and pins it. No operator toggle ships — the
// browser-native media query IS the honoring.)
//
//   node static/admin/scripts/api/__tests__/reduced-motion.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const css = fs.readFileSync(
  path.resolve(__dirname, '..', '..', '..', 'styles', 'tokens.css'),
  'utf8',
);

test('tokens.css honors prefers-reduced-motion with a universal suppression rule', () => {
  const i = css.indexOf('@media (prefers-reduced-motion');
  assert.ok(i !== -1, 'tokens.css must carry a prefers-reduced-motion media query');
  const block = css.slice(i, i + 400);
  assert.match(block, /\*,\s*\*::before,\s*\*::after/, 'must use the universal selector (covers themed CSS too)');
  assert.match(block, /transition-duration:\s*0\.01ms\s*!important/, 'transitions collapsed');
  assert.match(block, /animation-duration:\s*0\.01ms\s*!important/, 'animations collapsed');
  assert.match(block, /scroll-behavior:\s*auto\s*!important/, 'smooth-scroll disabled (#307)');
});
