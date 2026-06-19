// Regression pin for #300 — InvalidEvent error-translation surfaced a
// moderation-takedown message ("TakedownRecord needs a record subject…") for a
// runtime-setting save failure. InvalidEvent is overloaded (moderation
// subject-shape check AND setRuntimeSetting unknown-key), and the substrate
// sends a specific `message` in both arms, so the translation table must NOT
// canned-override InvalidEvent — it must fall through to the substrate message.
//
// Static-source pin. No framework —
//   node static/admin/scripts/api/__tests__/invalidevent-translation.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS_DIR = path.resolve(__dirname, '..', '..');
const src = fs.readFileSync(path.join(SCRIPTS_DIR, 'api/error-translations.js'), 'utf8');

test('InvalidEvent has no canned translation (falls through to substrate message)', () => {
  // The TABLE must not key InvalidEvent to a fixed string — that destroys the
  // substrate's informative message ("unknown runtime setting key …" etc.).
  assert.doesNotMatch(
    src,
    /InvalidEvent\s*:\s*\n?\s*"/,
    'InvalidEvent must not be a canned TABLE entry (it is overloaded; defer to the substrate message)',
  );
});

test('translate() falls back to the supplied message for unknown/untranslated codes', () => {
  // The fallback path (used for InvalidEvent now) returns the server message.
  assert.match(src, /return\s+fallback\s*!=\s*null\s*\?\s*fallback/, 'translate must fall back to the message arg');
});
