// Verification pin for #325 — Pride's random rainbow button-hover palette must
// pair every fill with a text color that clears WCAG 2.2 AA (4.5:1 for normal
// text). The palette lives in components/PrideHover.js; this reads it from
// source and checks each pair, so a regression (e.g. white text on yellow)
// fails the suite.
//
//   node static/admin/scripts/components/__tests__/pride-hover-contrast.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const src = fs.readFileSync(path.join(SCRIPTS_DIR, 'components', 'PrideHover.js'), 'utf8');

function channel(c) {
  const x = c / 255;
  return x <= 0.03928 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4);
}
function luminance(hex) {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}
function contrast(a, b) {
  const la = luminance(a);
  const lb = luminance(b);
  const hi = Math.max(la, lb);
  const lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

function palette() {
  const re = /fill:\s*'(#[0-9A-Fa-f]{6})'\s*,\s*text:\s*'(#[0-9A-Fa-f]{6})'/g;
  const out = [];
  let m;
  while ((m = re.exec(src)) !== null) out.push({ fill: m[1], text: m[2] });
  return out;
}

test('the hover palette has the six rainbow stripes', () => {
  assert.equal(palette().length, 6, 'six-stripe rainbow');
});

test('every hover fill/text pair clears WCAG AA (4.5:1)', () => {
  for (const { fill, text } of palette()) {
    const ratio = contrast(fill, text);
    assert.ok(ratio >= 4.5, `${text} on ${fill} = ${ratio.toFixed(2)}:1 (need 4.5:1)`);
  }
});
