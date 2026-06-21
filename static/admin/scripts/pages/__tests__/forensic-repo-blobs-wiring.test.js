// Static pin for the forensic-export repo/blobs content (#339). The substrate
// now ships repo.car + blobs/ in the bundle, so the modal's repo + blob
// checkboxes are no longer "deferred to v0.3" — they wire to includeRepo /
// includeBlobs and the deferral labels are gone.
//
//   node static/admin/scripts/pages/__tests__/forensic-repo-blobs-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const src = fs.readFileSync(
  path.resolve(__dirname, '..', 'AccountDetail.js'),
  'utf8',
);

test('the repo + blobs checkboxes no longer carry a deferral label', () => {
  assert.ok(!/deferred to v0\.3/i.test(src), 'no "deferred to v0.3" text remains');
});

test('the forensic modal wires includeRepo + includeBlobs to the checkboxes', () => {
  assert.ok(src.includes('id="fx-repo"'), 'repo checkbox present');
  assert.ok(src.includes('id="fx-blobs"'), 'blobs checkbox present');
  assert.ok(/includeRepo:\s*document\.getElementById\('fx-repo'\)\.checked/.test(src),
    'includeRepo bound to fx-repo');
  assert.ok(/includeBlobs:\s*document\.getElementById\('fx-blobs'\)\.checked/.test(src),
    'includeBlobs bound to fx-blobs');
});
