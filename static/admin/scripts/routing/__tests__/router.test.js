// Static-source pin against innerHTML XSS sinks in the admin
// router.
//
// The router previously interpolated URL-derived input
// (window.location.hash) into innerHTML in three places — the not-
// found, forbidden, and error fallback pages. A successful XSS in
// the admin UI hands the attacker the operator's
// localStorage 'aurora-admin-token' and from there every admin XRPC the
// operator has scope for, so this is privilege escalation, not
// defacement.
//
// Post-fix: the router renders all dynamic content through
// textContent on freshly-constructed DOM elements; no `innerHTML =`
// assignment survives. This test pins that property by reading the
// router source and asserting:
//
//   1. No `.innerHTML =` (or `.innerHTML+=`) appears anywhere in
//      the routing module.
//   2. No `outerHTML` / `insertAdjacentHTML` / `document.write` —
//      the same class of sink, easy to reach for by reflex.
//   3. The render path uses `textContent` (positive proof the fix
//      shape is in place, not just that the bad APIs were deleted).
//   4. URL-derived values (those reached via `window.location.hash`,
//      `parseHash`, or any extracted hash path) never get
//      concatenated with HTML-shaped string literals — catches the
//      "string-builder XSS" reintroduction even if no innerHTML
//      sink is present.
//
// Behavioral / DOM-mock testing was considered (route to a URL
// with an XSS payload, assert no script execution and that the
// rendered output's textContent equals the payload literally).
// Skipped because the routing module has no DOM-mock test
// infrastructure today and the static pin captures the same
// regression class with substantially less harness. Behavioral
// coverage is a v0.3 candidate alongside a broader UI-level XSS
// test sweep.
//
// No framework dependency — runs under bare Node via `node
// static/admin/scripts/routing/__tests__/router.test.js`.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const ROUTING_DIR = path.resolve(__dirname, '..');

function readModule(name) {
  return fs.readFileSync(path.join(ROUTING_DIR, name), 'utf8');
}

// Strip line- and block-comments before pattern-matching for sinks.
// A comment that mentions `innerHTML` for documentation purposes
// (which the post-fix router does — explaining *why* it doesn't use
// innerHTML) is fine; a real assignment is not.
function stripComments(src) {
  // Block comments first so /* contains // */ doesn't break.
  let out = src.replace(/\/\*[\s\S]*?\*\//g, '');
  // Line comments next.
  out = out.replace(/\/\/[^\n]*/g, '');
  return out;
}

test('router.js has no innerHTML / outerHTML / insertAdjacentHTML assignments', () => {
  const src = stripComments(readModule('router.js'));
  for (const sink of [/\.innerHTML\s*[+]?=/, /\.outerHTML\s*=/, /insertAdjacentHTML\s*\(/, /document\.write\s*\(/]) {
    assert.doesNotMatch(
      src,
      sink,
      `router.js must not contain ${sink.source} — use textContent + createElement instead`,
    );
  }
});

test('routes.js stays a pure data module (no HTML sinks anywhere)', () => {
  const src = stripComments(readModule('routes.js'));
  for (const sink of [/\.innerHTML\s*=/, /\.outerHTML\s*=/, /insertAdjacentHTML\s*\(/]) {
    assert.doesNotMatch(src, sink);
  }
});

test('router.js render path uses textContent (positive proof of the fix)', () => {
  const src = readModule('router.js');
  // textContent is reached for via createTextNode and direct .textContent =.
  // Either form counts.
  assert.match(
    src,
    /textContent\s*=|createTextNode\s*\(/,
    'router.js must use textContent / createTextNode for dynamic content',
  );
});

test('router.js does not concatenate hash-derived values with HTML literals', () => {
  const src = stripComments(readModule('router.js'));

  // Look for any HTML-shaped string literal (contains <tag) that's
  // adjacent to a + concatenation operator. The pre-fix shape
  // (`'<p>' + path + '</p>'`) was a textbook example. Catches
  // reintroduction even when innerHTML isn't the destination.
  //
  // The pattern allows static HTML literals that aren't part of a
  // concat expression (e.g. inside comments, which we already
  // stripped, or as right-hand-side of a single static assignment).
  // A literal followed by a `+` flags as a concat; same for `+`
  // followed by a literal.
  const concatPatterns = [
    /'[^']*<[a-z][^']*'\s*\+/i, // '<tag>...' +
    /\+\s*'[^']*<[a-z][^']*'/i, // + '<tag>...'
    /"[^"]*<[a-z][^"]*"\s*\+/i, // double-quoted variants
    /\+\s*"[^"]*<[a-z][^"]*"/i,
    /`[^`]*<[a-z][^`]*\$\{/i, // template literal with HTML + interpolation
  ];
  for (const pat of concatPatterns) {
    assert.doesNotMatch(
      src,
      pat,
      `router.js must not concatenate HTML-shaped literals with dynamic values: ${pat.source}`,
    );
  }
});

test('router.js renders untrusted hash content as text (regression: notFound path)', () => {
  // The most attacker-relevant render path: mountNotFound receives
  // the URL hash (controllable by anyone with a link). The post-fix
  // shape uses { tag: 'code', text: '#' + (path || '') } passed
  // through renderMessage, which routes the text through
  // element.textContent. Pin that intent by structural match.
  const src = readModule('router.js');
  // Must build a <code> element AND write text through textContent
  // somewhere in the same module. Loose structural check — exact
  // call shape is allowed to evolve as long as the textContent
  // discipline holds.
  assert.match(
    src,
    /createElement\(\s*['"]code['"]\s*\)|tag:\s*['"]code['"]/,
    "router.js must construct a <code> element for the not-found path display",
  );
  assert.match(
    src,
    /textContent/,
    "router.js must route dynamic content through textContent",
  );
});
