// Static pins for the §5.5.4 Phase D escalation-rules UI (#348): the rule
// list, the add/edit form with conditional per-trigger params + action-type
// selector, the three trigger types, soft-delete + show-deleted, and the CRUD
// + clearEscalation endpoint wrappers.
//
//   node static/admin/scripts/pages/__tests__/escalation-rules-phase-d.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'ConfigModerationPolicy.js'), 'utf8');

test('CRUD + clearEscalation endpoint wrappers exist', () => {
  const ep = fs.readFileSync(path.join(PAGES, '..', 'api', 'endpoints.js'), 'utf8');
  for (const m of ['createEscalationRule', 'editEscalationRule', 'deleteEscalationRule', 'listEscalationRules', 'clearEscalation']) {
    assert.ok(ep.includes(m + ':'), 'wrapper ' + m);
    assert.ok(ep.includes('tools.aurora.superadmin.' + m), 'NSID for ' + m);
  }
});

test('the three trigger types + their conditional param blocks', () => {
  for (const t of ['report-count', 'operator-action', 'category-match']) {
    assert.ok(src.includes('value="' + t + '"'), 'trigger option ' + t);
    assert.ok(src.includes('mod-esc-params-' + t), 'param block ' + t);
  }
  assert.ok(/syncEscParamsVisibility/.test(src), 'conditional param reveal');
});

test('the two action types', () => {
  for (const a of ['mark', 'reassign-to-superadmin']) {
    assert.ok(src.includes('value="' + a + '"'), 'action option ' + a);
  }
});

test('create/edit/delete/list escalation flows wired', () => {
  assert.ok(src.includes('createEscalationRule(body)'), 'create');
  assert.ok(src.includes('editEscalationRule(body)'), 'edit');
  assert.ok(src.includes('deleteEscalationRule({ id:'), 'delete');
  assert.ok(src.includes('listEscalationRules({ includeDeleted'), 'list');
  assert.ok(/mod-esc-show-deleted/.test(src), 'show-deleted toggle');
});

test('delete is a destructive soft-delete confirm', () => {
  assert.ok(/Delete escalation rule/.test(src) && /Soft-delete this rule/.test(src), 'soft-delete confirm');
});

test('queue page has escalated/orphan affordance + de-escalate (§5.5 MD-43)', () => {
  const q = fs.readFileSync(path.join(PAGES, 'Queue.js'), 'utf8');
  assert.ok(/Escalated, awaiting assignment/.test(q), 'orphan marker');
  assert.ok(/mod-deescalate/.test(q) && /clearEscalation\(/.test(q), 'de-escalate button → clearEscalation');
  assert.ok(/isEscalated && isSuper/.test(q), 'de-escalate gated SuperAdmin + escalated');
});
