// Regression pin for #302 — getReport hit an unregistered NSID.
//
// The report-detail page called `com.atproto.admin.getReport` (legacy Bluesky
// NSID), which the substrate never registered → 404 on every report. #302
// registers `tools.aurora.admin.getReport` (the Aurora namespace) and points
// the wrapper at it. This pins the wrapper↔route NSID agreement.
//
// Static-source pin. No framework —
//   node static/admin/scripts/api/__tests__/getreport-nsid.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const src = fs.readFileSync(path.join(SCRIPTS_DIR, 'api/endpoints.js'), 'utf8');

test('getReport calls the registered tools.aurora.admin.getReport NSID', () => {
  const line = src.split('\n').find((l) => l.includes('getReport:'));
  assert.ok(line, 'getReport wrapper must exist');
  assert.match(line, /tools\.aurora\.admin\.getReport/, 'must call the registered Aurora NSID');
  assert.doesNotMatch(line, /com\.atproto\.admin\.getReport/, 'must not call the unregistered legacy NSID (404)');
});
