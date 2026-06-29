// Static pins for the §5.5.4 Phase C auto-label-rules UI (#347): the rule
// list, the add/edit form with conditional per-trigger params, the three
// trigger types, soft-delete + show-deleted, and the CRUD endpoint wrappers.
//
//   node static/admin/scripts/pages/__tests__/auto-label-rules-phase-c.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'ConfigModerationPolicy.js'), 'utf8');

test('CRUD endpoint wrappers exist', () => {
  const ep = fs.readFileSync(path.join(PAGES, '..', 'api', 'endpoints.js'), 'utf8');
  for (const m of ['createAutoLabelRule', 'editAutoLabelRule', 'deleteAutoLabelRule', 'listAutoLabelRules']) {
    assert.ok(ep.includes(m + ':'), 'wrapper ' + m);
    assert.ok(ep.includes('tools.aurora.superadmin.' + m), 'NSID for ' + m);
  }
});

test('the three trigger types + their conditional param blocks', () => {
  for (const t of ['report-count', 'operator-action', 'account-age-activity']) {
    assert.ok(src.includes('value="' + t + '"'), 'trigger option ' + t);
    assert.ok(src.includes('mod-rule-params-' + t), 'param block ' + t);
  }
  assert.ok(/syncRuleParamsVisibility/.test(src), 'conditional param reveal');
});

test('operator-action trigger offers the 16 emit_event action types', () => {
  assert.ok(/OPERATOR_ACTION_TYPES\s*=\s*\[/.test(src), 'action-type list present');
  for (const a of ['TakedownAccount', 'ApplyLabel', 'ResolveReport', 'UpdateSubjectStatus']) {
    assert.ok(src.includes("'" + a + "'"), 'action type ' + a);
  }
});

test('create/edit/delete/list rule flows wired', () => {
  assert.ok(src.includes('createAutoLabelRule(body)'), 'create flow');
  assert.ok(src.includes('editAutoLabelRule(body)'), 'edit flow (form reused with edit-id)');
  assert.ok(src.includes('deleteAutoLabelRule({ id:'), 'delete flow');
  assert.ok(src.includes('listAutoLabelRules({ includeDeleted'), 'list with include_deleted');
  assert.ok(/mod-rules-show-deleted/.test(src), 'show-deleted toggle');
});

test('delete is a destructive confirm; soft-delete framing', () => {
  assert.ok(/destructiveConfirm/.test(src) && /Soft-delete this rule/.test(src), 'soft-delete confirm');
});

test('label value required before save', () => {
  assert.ok(/Label value is required/.test(src), 'label-required guard');
});
