// Regression pin for #308 — required rationale is overkill for cosmetic settings.
//
// AuroraAuditedSave gains a `cosmetic` opt-out: a light confirm, no required
// rationale + no typed-confirm gate, but the setting is still written (so the
// audit entry still lands) under an auto-filled rationale. ConfigThemes' set-
// deployment-default uses it (theme is cosmetic) while still writing
// theme.deployment-default.
//
//   node static/admin/scripts/api/__tests__/cosmetic-save.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.join(SCRIPTS_DIR, rel), 'utf8');

test('AuroraAuditedSave honors the cosmetic opt-out (drops required rationale, auto-fills)', () => {
  const src = read('components/AuroraAuditedSave.js');
  assert.match(src, /rationaleRequired:\s*!cosmetic/, 'cosmetic must drop the required-rationale gate');
  assert.match(src, /autoRationale/, 'cosmetic must auto-fill a rationale (so the audit entry still lands)');
});

test('ConfigThemes set-default is a cosmetic save but still writes the setting', () => {
  const src = read('pages/ConfigThemes.js');
  const i = src.indexOf('function setDefault');
  assert.ok(i !== -1, 'setDefault must exist');
  const fn = src.slice(i, i + 700);
  assert.match(fn, /cosmetic:\s*true/, 'theme deployment-default is a cosmetic save (no rationale friction)');
  assert.match(fn, /theme\.deployment-default/, 'still writes the setting → audit entry still emitted');
});
