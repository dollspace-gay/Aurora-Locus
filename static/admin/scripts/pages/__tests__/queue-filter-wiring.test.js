// Static pins for the restored Queue header status filter (#209). Guards the
// frontend↔backend contract: the FilterStrip status facet, the param the page
// sends to getModerationQueue, and the URL-state round-trip — the wiring whose
// absence made the prior filter a no-op (the reason #209 was filed).
//
//   node static/admin/scripts/pages/__tests__/queue-filter-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const queueSrc = fs.readFileSync(path.join(PAGES, 'Queue.js'), 'utf8');

test('Queue builds a FilterStrip with the Status facet and its backed options', () => {
  assert.ok(queueSrc.includes('AuroraFilterStrip'), 'uses the FilterStrip component');
  assert.ok(/id:\s*'status'/.test(queueSrc), 'status facet id');
  // Every option maps to a real backend value: the four ReportStatus variants
  // plus the explicit "all" that clears the filter server-side.
  for (const v of ['open', 'acknowledged', 'escalated', 'resolved', 'all']) {
    assert.ok(queueSrc.includes("value: '" + v + "'"), 'status option ' + v);
  }
});

test('Queue sends the status param to getModerationQueue', () => {
  assert.ok(queueSrc.includes('getModerationQueue(params)'), 'calls endpoint with built params');
  assert.ok(queueSrc.includes('params.status = lastFilters.status'), 'wires status into the request');
});

test('Queue round-trips status through the shared URL-state shape', () => {
  assert.ok(queueSrc.includes("SCALAR_KEYS = ['status']"), 'status is a URL scalar key');
  assert.ok(queueSrc.includes('AuroraListPage.readFilters'), 'restores filters from URL on mount');
  assert.ok(queueSrc.includes('AuroraListPage.applyFilters'), 'writes filters to URL on apply');
  assert.ok(/readFilters\([^)]*\{\s*status:\s*'open'\s*\}/.test(queueSrc), "defaults to status 'open'");
});

test('Queue distinguishes a filtered miss from a genuinely empty queue', () => {
  assert.ok(queueSrc.includes('filterActive'), 'computes filter-active state');
  assert.ok(queueSrc.includes('queue-clear-filters'), 'offers a clear-filters affordance when filtered');
});
