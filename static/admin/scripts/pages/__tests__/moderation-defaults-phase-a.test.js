// Static pins for the §5.5.4 Phase A "configurable moderation defaults"
// UI (#345): the default-action control, the conditional per-category
// map editor over the six ReportReason values, and the stale-hold field —
// all SuperAdmin-gated and wired to the three moderation.defaults.* runtime
// settings. Guards the wiring + the by-category conditional + the empty-map
// guard.
//
//   node static/admin/scripts/pages/__tests__/moderation-defaults-phase-a.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'ConfigModerationPolicy.js'), 'utf8');

test('hosts the three Phase A default settings, read + write', () => {
  for (const key of [
    'moderation.defaults.report-action',
    'moderation.defaults.report-action-category-map',
    'moderation.defaults.hide-pending-review-stale-days',
  ]) {
    assert.ok(src.includes("getRuntimeSetting('" + key + "')"), 'reads ' + key);
    assert.ok(src.includes("key: '" + key + "'"), 'writes ' + key);
  }
});

test('default-action control offers the three §2.2 values', () => {
  for (const v of ['acknowledge', 'hide-pending-review', 'auto-resolve-by-category']) {
    assert.ok(src.includes('value="' + v + '"'), 'default-action option ' + v);
  }
});

test('per-category editor covers the six ReportReason categories', () => {
  // The category list drives the conditional map editor.
  assert.ok(
    /REPORT_CATEGORIES\s*=\s*\['spam',\s*'violation',\s*'misleading',\s*'sexual',\s*'rude',\s*'other'\]/.test(src),
    'the six categories are enumerated'
  );
  // Map editor is only shown for the by-category action.
  assert.ok(/auto-resolve-by-category/.test(src) && /syncCategoryMapVisibility/.test(src),
    'category map visibility is gated on the by-category action');
});

test('by-category with an empty map is blocked client-side (§2.2)', () => {
  assert.ok(/Object\.keys\(map\)\.length === 0/.test(src), 'guards the empty-map misconfiguration');
});

test('stale-hold field is bounded 1..365', () => {
  assert.ok(/min="1"/.test(src) && /max="365"/.test(src), 'stale-days input bounds');
  assert.ok(/staleRaw >= 1 && staleRaw <= 365/.test(src), 'stale-days validated on save');
});

test('SuperAdmin-gated controls', () => {
  assert.ok(src.includes("session.hasRole('superadmin')"), 'resolves SuperAdmin');
  assert.ok(/mod-defaults-save/.test(src), 'save button id present');
  // The save button only renders for SuperAdmin (isSuper ternary).
  assert.ok(/isSuper \? '<button[^']*mod-defaults-save/.test(src), 'save button is SuperAdmin-gated');
});
