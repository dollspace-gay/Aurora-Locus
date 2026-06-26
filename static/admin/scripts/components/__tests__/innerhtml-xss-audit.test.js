// XSS call-site audit pins (#358). The §15 inventory flagged the innerHTML
// insertion points in Modal.js / PaginationStrip.js (Drawer.js was already
// hardened — no innerHTML remains). Audit conclusion: ZERO real XSS surfaces.
// Dynamic content reaching these sinks is either escaped (Modal title; the
// form()/destructiveConfirm() helpers esc() every interpolation) or fed via a
// Node (appended, not innerHTML'd), or an unfed parameter (PaginationStrip
// info; Modal string-footer). The one live raw-string `open({body})` caller —
// ConfigThemes' validation-errors dialog — pre-escapes each error. These pins
// guard that those invariants don't silently regress.
//
//   node --test static/admin/scripts/components/__tests__/innerhtml-xss-audit.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..');
const read = (rel) => fs.readFileSync(path.resolve(ROOT, rel), 'utf8');

const modal = read('components/Modal.js');
const pagination = read('components/PaginationStrip.js');
const configThemes = read('pages/ConfigThemes.js');

test('Drawer.js has no innerHTML sink (already hardened)', () => {
  const drawer = read('components/Drawer.js');
  assert.ok(!/\.innerHTML\s*=/.test(drawer), 'Drawer.js must not assign innerHTML');
});

test('Modal escapes the title in the shell', () => {
  // The shell interpolates only the title, and it goes through esc().
  assert.ok(
    /id="'\s*\+\s*titleId\s*\+\s*'">'\s*\+\s*\(global\.AuroraDom\s*\?\s*global\.AuroraDom\.esc\(title\)/.test(modal),
    'modal shell title must be esc()-wrapped',
  );
});

test('Modal form()/destructiveConfirm() escape their interpolations', () => {
  // Body text + labels + options + gate + ack all go through esc().
  assert.ok(modal.includes("esc(spec.body)"), 'helper body text is escaped');
  assert.ok(modal.includes("esc(f.label"), 'field labels escaped');
  assert.ok(modal.includes("esc(opt.label)") && modal.includes("esc(opt.value)"), 'select options escaped');
  assert.ok(modal.includes("esc(typedGate)"), 'typed-gate string escaped');
});

test('the lone raw-string open({body}) caller (ConfigThemes) pre-escapes', () => {
  // ConfigThemes builds the validation-errors body as a string and passes it
  // to AuroraModal.open — it MUST esc() each error before insertion.
  assert.ok(
    /errs\.map\(\(e\)\s*=>\s*'<li>'\s*\+\s*esc\(e\)\s*\+\s*'<\/li>'\)/.test(configThemes),
    'ConfigThemes validation-errors body must esc() each error',
  );
});

test('PaginationStrip info has no unescaped caller-supplied feed', () => {
  // The only dynamic field is spec.info; the audit confirmed no caller passes
  // it. Guard: no page calls AuroraPagination.render with an `info:` field.
  const pagesDir = path.resolve(ROOT, 'pages');
  const offenders = [];
  for (const f of fs.readdirSync(pagesDir)) {
    if (!f.endsWith('.js')) continue;
    const src = fs.readFileSync(path.join(pagesDir, f), 'utf8');
    if (/AuroraPagination\.render\(/.test(src) && /\binfo:/.test(src)) offenders.push(f);
  }
  assert.deepEqual(offenders, [], 'no page should pass an info: field to AuroraPagination.render without an esc review');
  // And the component documents the raw-insert contract.
  assert.ok(pagination.includes('pre-escape'), 'PaginationStrip documents the info raw-insert contract');
});
