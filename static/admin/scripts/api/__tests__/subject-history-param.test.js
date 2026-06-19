// Regression pin for #301 — Subject context/history drawers passed the wrong
// query param name. The backend GetSubjectContextParams and
// GetSubjectHistoryParams both declare `pub did: String` (camelCase → `did`);
// the AccountDetail callsites passed `{ subjectDid: ... }` → 400 "missing field
// `did`". Both callsites must pass `did`.
//
// Static-source pin. No framework —
//   node static/admin/scripts/api/__tests__/subject-history-param.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES_DIR = path.resolve(__dirname, '..', '..', 'pages');
const src = fs.readFileSync(path.join(PAGES_DIR, 'AccountDetail.js'), 'utf8');

test('getSubjectContext is called with `did`, not `subjectDid`', () => {
  const call = src.split('\n').find((l) => l.includes('getSubjectContext('));
  assert.ok(call, 'getSubjectContext callsite must exist');
  assert.match(call, /\{\s*did:/, 'must pass { did: ... } (backend field is `did`)');
  assert.doesNotMatch(call, /subjectDid:/, 'must NOT pass subjectDid (400 missing field `did`)');
});

test('getSubjectHistory is called with `did`, not `subjectDid`', () => {
  const call = src.split('\n').find((l) => l.includes('getSubjectHistory('));
  assert.ok(call, 'getSubjectHistory callsite must exist');
  assert.match(call, /\{\s*did:/, 'must pass { did: ... } (backend field is `did`)');
  assert.doesNotMatch(call, /subjectDid:/, 'must NOT pass subjectDid (400 missing field `did`)');
});
