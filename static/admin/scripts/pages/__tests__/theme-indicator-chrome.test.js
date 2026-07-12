// Pins the theme-state-indicator chrome unification (#441): the "Active Theme"
// and "Deployment default" indicators adopt the .btn-sm button chrome (same
// padding, radius, font size) so they line up with the "Use this theme" /
// "Set as deployment default" buttons — differentiated by a muted fill + bold
// text, NOT a different shape — and stay non-interactive.
//
//   node static/admin/scripts/pages/__tests__/theme-indicator-chrome.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const css = fs.readFileSync(
  path.resolve(__dirname, '..', '..', '..', 'styles', 'pages.css'),
  'utf8',
);
const cfg = fs.readFileSync(path.resolve(__dirname, '..', 'ConfigThemes.js'), 'utf8');

// The flat CSS rule body for `selector` (up to its closing brace).
function ruleBody(selector) {
  const i = css.indexOf(selector);
  assert.ok(i !== -1, selector + ' rule must exist');
  return css.slice(i, css.indexOf('}', i));
}

test('the indicators adopt the .btn-sm chrome (same padding / radius / font size)', () => {
  const shared = ruleBody('.theme-active-pill,\n.theme-default-pill');
  assert.match(shared, /padding:\s*0\.375rem\s+0\.75rem/, 'same padding as .btn-sm');
  assert.match(shared, /border-radius:\s*var\(--radius-sm\)/, 'same radius as buttons (not the chip --radius-full)');
  assert.match(shared, /font-size:\s*0\.8125rem/, 'same font size as .btn-sm');
  assert.match(shared, /font-weight:\s*var\(--font-weight-bold\)/, 'bold text is the differentiator');
  assert.match(shared, /cursor:\s*default/, 'non-interactive: not a button cursor');
});

test('the indicators are no longer the tiny --radius-full chips', () => {
  const chips = ruleBody('.theme-mode-pill,\n.theme-aaa-pill');
  assert.match(chips, /--radius-full/, 'the small Mode/AAA chips keep the pill radius');
  assert.ok(!chips.includes('theme-active-pill'), 'the state indicators are split out of the tiny-chip rule');
});

test('the action column stretches so buttons + indicators share a width', () => {
  const action = ruleBody('.theme-row-action ');
  assert.match(action, /align-items:\s*stretch/, 'the stacked actions stretch to one shared width');
});

test('the action column has a fixed min-width so widths match across cards', () => {
  // Without this the active card (indicators only) is narrower than the button
  // cards; the min-width pins every card's action column to one footprint (#441).
  const action = ruleBody('.theme-row-action ');
  assert.match(action, /min-width:\s*\d/, 'a min-width is set on the action column');
});

test('the indicators are non-interactive <span>s (never buttons)', () => {
  assert.match(cfg, /<span class="theme-active-pill">Active Theme<\/span>/, 'Active Theme is a span');
  assert.match(cfg, /<span class="theme-default-pill">Deployment default<\/span>/, 'Deployment default is a span');
});
