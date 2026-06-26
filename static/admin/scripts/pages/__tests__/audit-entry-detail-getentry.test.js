// Static pins for the _auditCache retirement (#359). The audit detail page
// used to read a page-scoped window._auditCache populated by the Audit list;
// a miss degraded to "narrow with filters" and the chain-walk couldn't leave
// the loaded page. It now fetches each entry server-side via getAuditEntry
// (by id for the detail view, by hash for walk-to-previous), and the list no
// longer seeds a global. These guard that the global is gone and the endpoint
// wiring is in place.
//
//   node static/admin/scripts/pages/__tests__/audit-entry-detail-getentry.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS = path.resolve(__dirname, '..', '..');
const detail = fs.readFileSync(path.resolve(SCRIPTS, 'pages', 'AuditEntryDetail.js'), 'utf8');
const auditList = fs.readFileSync(path.resolve(SCRIPTS, 'pages', 'Audit.js'), 'utf8');
const endpoints = fs.readFileSync(path.resolve(SCRIPTS, 'api', 'endpoints.js'), 'utf8');

// Strip line/block comments so "no _auditCache" pins test code, not prose that
// documents the removal.
function stripComments(src) {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

test('endpoints registers getAuditEntry against the admin NSID', () => {
  assert.ok(
    /getAuditEntry:\s*\(params\)\s*=>\s*C\(\)\.get\('tools\.aurora\.admin\.getAuditEntry'/.test(endpoints),
    'getAuditEntry GETs tools.aurora.admin.getAuditEntry',
  );
});

test('window._auditCache is fully retired (no code references)', () => {
  assert.ok(!/_auditCache/.test(stripComments(detail)), 'AuditEntryDetail has no _auditCache code');
  assert.ok(!/_auditCache/.test(stripComments(auditList)), 'Audit list no longer writes _auditCache');
});

test('the detail view fetches the entry by id from the server', () => {
  assert.ok(
    /getAuditEntry\(\{\s*id:\s*id\s*\}\)/.test(detail),
    'loadEntry calls getAuditEntry({ id })',
  );
});

test('walk-to-previous resolves the prior entry by hash', () => {
  assert.ok(
    /getAuditEntry\(\{\s*hash:\s*prevHash\s*\}\)/.test(detail),
    'walkChainTo calls getAuditEntry({ hash: prevHash })',
  );
});

test('a 404 renders an empty state rather than a retry boundary', () => {
  assert.ok(
    /e\.status === 404/.test(detail),
    'loadEntry distinguishes a 404 from a transport failure',
  );
});
