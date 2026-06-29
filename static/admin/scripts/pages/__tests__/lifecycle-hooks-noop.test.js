// Static pin for the §11.8 lifecycle-hook declaration-aware no-op (frontend
// surface). The substrate detects hooks a theme declares and ships them as the
// listInstalled `declaredLifecycleHooks` field WITHOUT executing them (§11.8.4
// — execution waits on a security-reviewed sandbox). The themes page must
// surface declared hooks as dormant, never run them. This guards that the
// renderer reads the field and frames it as not-run, and that no execution path
// (fetch/eval/import of a hook script) crept into the page.
//
//   node static/admin/scripts/pages/__tests__/lifecycle-hooks-noop.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const themesSrc = fs.readFileSync(path.join(PAGES, 'ConfigThemes.js'), 'utf8');

test('themes page surfaces declared lifecycle hooks from the listInstalled field', () => {
  assert.ok(themesSrc.includes('declaredLifecycleHooks'), 'reads the declaredLifecycleHooks field');
  assert.ok(/Lifecycle hooks/.test(themesSrc), 'renders a lifecycle-hooks line');
  assert.ok(/not run in this version/i.test(themesSrc), 'frames the hooks as declared-but-dormant');
});

test('themes page does not execute hook scripts (no-op layer only)', () => {
  // The no-op layer must not fetch/eval/import a theme-declared script. Guard
  // the obvious execution sinks against a future edit that "wires it up" here
  // instead of behind the (still-absent) sandbox.
  assert.ok(!/\beval\s*\(/.test(themesSrc), 'no eval()');
  assert.ok(!/new\s+Function\s*\(/.test(themesSrc), 'no Function constructor');
  assert.ok(!/import\s*\(/.test(themesSrc), 'no dynamic import()');
  assert.ok(!/\.script\b/.test(themesSrc), 'does not dereference a hook script reference');
});
