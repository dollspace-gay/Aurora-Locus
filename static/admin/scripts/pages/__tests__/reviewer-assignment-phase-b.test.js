// Static pins for the §5.5.4 Phase B reviewer-assignment UI (#346): the
// mode selector, the conditional per-category operator-pool editor, the
// empty-pool warning, the mode-change banner (localStorage-dismissed via
// the versioned key), and the assignReviewer endpoint wrapper.
//
//   node static/admin/scripts/pages/__tests__/reviewer-assignment-phase-b.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'ConfigModerationPolicy.js'), 'utf8');

test('hosts the reviewer-assignment settings, read + write', () => {
  assert.ok(src.includes("getRuntimeSetting('moderation.defaults.reviewer-assignment-mode')"), 'reads mode');
  assert.ok(src.includes("getRuntimeSetting('moderation.defaults.reviewer-routing-category-map')"), 'reads map');
  assert.ok(src.includes("getRuntimeSetting('moderation.defaults.reviewer-mode-version')"), 'reads version');
  assert.ok(src.includes("key: 'moderation.defaults.reviewer-assignment-mode'"), 'writes mode');
  assert.ok(src.includes("key: 'moderation.defaults.reviewer-routing-category-map'"), 'writes map');
});

test('mode selector offers the four §4.2 modes', () => {
  for (const v of ['manual', 'round-robin', 'load-balanced', 'category-routed']) {
    assert.ok(src.includes('value="' + v + '"'), 'mode option ' + v);
  }
});

test('per-category pool editor + empty-pool warning, gated on category-routed', () => {
  assert.ok(/mod-reviewer-pool/.test(src), 'per-category pool inputs');
  assert.ok(/syncReviewerMapVisibility/.test(src) && /category-routed/.test(src), 'map gated on by-category mode');
  assert.ok(/mod-reviewer-empty-warn/.test(src) && /Empty pool/.test(src), 'empty-pool warning');
  assert.ok(/Object\.keys\(map\)\.length === 0/.test(src), 'blocks by-category with no pools');
});

test('mode-change banner uses the versioned localStorage dismissal key (§4.5)', () => {
  assert.ok(/aurora\.banner-dismissed\.queue-assignment-mode-change\.v/.test(src), 'versioned dismissal key');
  assert.ok(/localStorage\.getItem/.test(src) && /localStorage\.setItem/.test(src), 'dismissal persists per operator');
});

test('assignReviewer endpoint wrapper exists', () => {
  const ep = fs.readFileSync(path.join(PAGES, '..', 'api', 'endpoints.js'), 'utf8');
  assert.ok(/assignReviewer:\s*\(body\)\s*=>/.test(ep), 'superadmin.assignReviewer wrapper');
  assert.ok(ep.includes('tools.aurora.superadmin.assignReviewer'), 'targets the right NSID');
});
