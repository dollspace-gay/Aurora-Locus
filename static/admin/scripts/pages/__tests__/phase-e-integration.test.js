// Static pins for §5.5.4 Phase E (#349): the composite-load wrapper, the
// audit-log source + Operator-rule-management filters with mutual exclusivity
// (MD-44), and the lexicon-migration banner.
//
//   node static/admin/scripts/pages/__tests__/phase-e-integration.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (rel) => fs.readFileSync(path.join(PAGES, rel), 'utf8');

test('getDefaultsState composite-load wrapper exists', () => {
  const ep = read('../api/endpoints.js');
  assert.ok(/getDefaultsState:\s*\(\)\s*=>/.test(ep), 'wrapper');
  assert.ok(ep.includes('tools.aurora.superadmin.getDefaultsState'), 'NSID');
});

test('audit-log adds source + rule-management filters', () => {
  const a = read('Audit.js');
  assert.ok(/id: 'source'/.test(a), 'source filter');
  assert.ok(/id: 'ruleManagement'/.test(a), 'rule-management filter');
  // All 7 source values.
  for (const v of ['default_action', 'auto_label_rule', 'stale_expiration', 'operator_removal', 'escalation', 'system_diagnostic', 'manual']) {
    assert.ok(a.includes("value: '" + v + "'"), 'source value ' + v);
  }
  assert.ok(a.includes("SCALAR_KEYS = ['actor', 'subject', 'subjectCid', 'action', 'source']"), 'source url-state');
  assert.ok(/ruleManagement/.test(a) && /params\.ruleManagement = true/.test(a), 'rule-management param mapping');
});

test('source / rule-management are mutually exclusive (MD-44)', () => {
  const a = read('Audit.js');
  assert.ok(/vals\.source && vals\.ruleManagement/.test(a) && /vals\.ruleManagement = false/.test(a), 'mutual exclusivity clears the other');
  // Param mapping prefers rule-management when both somehow present.
  assert.ok(/if \(lastFilters\.ruleManagement\) params\.ruleManagement = true;\s*\n\s*else if \(lastFilters\.source\)/.test(a), 'one-of param application');
});

test('lexicon-migration banner on the moderation policy page (§6.4)', () => {
  const c = read('ConfigModerationPolicy.js');
  assert.ok(/mod-lexicon-banner/.test(c), 'banner element');
  assert.ok(c.includes("getRuntimeSetting('moderation.lexicon.migration-banner')"), 'reads the banner setting');
  assert.ok(/aurora\.banner-dismissed\.lexicon-migration\./.test(c), 'versioned localStorage dismissal');
  assert.ok(/prunedKeys/.test(c) && /flaggedRuleIds/.test(c), 'surfaces pruned keys + flagged rules');
});
