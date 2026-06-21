// Static pins for audience-oracle activity instrumentation (#335). The
// getOracleActivity endpoint now returns real aggregate consultation counts
// (instrumented:true) instead of the old instrumented:false stub; the Overview
// renders the write/read breakdown. These guard that the page reads the new
// shape and that the surface is framed as the audience oracle (a memory-#17
// translation of the design's "block/mute oracle" label, which has no v0.9
// substrate).
//
//   node static/admin/scripts/pages/__tests__/oracle-activity-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const src = fs.readFileSync(path.join(PAGES, 'KryphocronOverview.js'), 'utf8');
const i18n = JSON.parse(
  fs.readFileSync(path.resolve(PAGES, '..', '..', 'i18n', 'en.json'), 'utf8'),
);

test('Overview renders the audience-oracle consultation breakdown', () => {
  assert.ok(src.includes('o.consultations'), 'reads the consultations object');
  assert.ok(/c\.write|w\.allowed/.test(src), 'renders the write-path counts');
  assert.ok(/c\.read|r\.authorized/.test(src), 'renders the read-path counts');
  assert.ok(src.includes('o.instrumented'), 'still honours the instrumented flag');
});

test('the oracle block is labelled for the audience oracle', () => {
  const ov = i18n.kryphocron.overview;
  assert.match(ov.oracle_title, /audience/i, 'title names the audience oracle');
  for (const k of [
    'oracle_consultations',
    'oracle_write',
    'oracle_read',
    'oracle_allowed',
    'oracle_denied',
    'oracle_deferred',
    'oracle_authorized',
  ]) {
    assert.ok(typeof ov[k] === 'string' && ov[k].length, 'i18n key ' + k);
  }
});
