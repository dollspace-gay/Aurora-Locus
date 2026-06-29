// Static pins for v0.9 Integration hooks Phase A (#350): the ConfigIntegrationHooks
// page (CRUD form, execution-status banner, normalization disclosure, optimistic-
// concurrency token, escape discipline), the CRUD + composite-load endpoint
// wrappers, the retired stub, and the audit-log Integration-hook sibling filter
// (one-way clear per design-commit 26 + empty-intersection note per design-commit 34).
//
//   node static/admin/scripts/pages/__tests__/integration-hooks-phase-a.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (rel) => fs.readFileSync(path.join(PAGES, rel), 'utf8');
const page = read('ConfigIntegrationHooks.js');

test('registers the real page key; stub retired', () => {
  assert.ok(/register\('configIntegrationHooks'/.test(page), 'registers configIntegrationHooks');
  const stubs = read('ConfigStubs.js');
  assert.ok(!/key:\s*'configIntegrationHooks'/.test(stubs), 'stub row removed (real page owns the key)');
});

test('CRUD + composite-load endpoint wrappers', () => {
  const ep = read('../api/endpoints.js');
  for (const m of ['createHook', 'editHook', 'deleteHook', 'listHooks', 'getIntegrationHooksState']) {
    assert.ok(ep.includes(m + ':'), 'wrapper ' + m);
    assert.ok(ep.includes('tools.aurora.superadmin.' + m), 'NSID ' + m);
  }
});

test('execution-status banner + honest framing + normalization disclosure', () => {
  assert.ok(/hooks-exec-banner/.test(page), 'execution-status banner element');
  assert.ok(/not yet executed/.test(page), 'honest not-yet-executed framing');
  assert.ok(/stored in normalized form/.test(page), 'URL normalization disclosure (design-commit 35)');
});

test('optimistic-concurrency token threaded on edit', () => {
  assert.ok(/hook-edit-token/.test(page), 'last-modified token element');
  assert.ok(/expectedLastModifiedAt/.test(page), 'edit sends expectedLastModifiedAt');
});

test('escape discipline + URL truncation with title tooltip', () => {
  assert.ok(/AuroraDom\.esc/.test(page), 'uses the escape helper');
  assert.ok(/title="' \+ esc\(url\.slice\(0, 500\)\)/.test(page), 'truncated URL with escaped title tooltip (design-commit 24)');
});

test('event-class checkboxes from the substrate-provided available set', () => {
  assert.ok(/availableEventClasses/.test(page), 'reads available classes from composite-load');
  assert.ok(/hook-ec/.test(page), 'per-class checkboxes');
});

test('delete is one-way soft-delete confirm', () => {
  assert.ok(/one-way \(no restore\)/.test(page), 'one-way deletion framing (design-commit 21)');
});

test('audit-log Integration-hook filter: one-way clear + empty-intersection note', () => {
  const a = read('Audit.js');
  assert.ok(/id: 'hookManagement'/.test(a), 'hook filter control');
  // Selecting hook filter clears the §5.5.4 filters (one-way).
  assert.ok(/turnedOnHook/.test(a) && /vals\.source = ''/.test(a) && /vals\.ruleManagement = false/.test(a), 'one-way clear of §5.5.4 filters');
  assert.ok(/params\.hookManagement = true/.test(a), 'hook-management param mapping');
  assert.ok(/intersection is empty/.test(a), 'empty-intersection note (design-commit 34)');
});
