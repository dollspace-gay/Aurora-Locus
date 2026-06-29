// Behavioural test for sidebar nav active-state matching (#411).
//
// Bug: opening "Sequencer recovery" (route `ops/sequencer/recovery`) lit BOTH
// the Sequencer item (`ops/sequencer`) and the Sequencer recovery item, because
// the old matching was `route === path || path.startsWith(route + '/')` applied
// per-item — and on the child page the ancestor `ops/sequencer` also matched by
// prefix. The fix resolves the single active route as the LONGEST matching one,
// so a child lights only its own entry while a detail page with no nav entry of
// its own still lights its parent.
//
// `resolveActiveRoute` is a pure function (no DOM); both the router and app.js
// drive the highlight through it. The routing IIFE can't be `require`d under
// bare Node (it touches `window`/`document` at load), so — matching the static
// convention of router.test.js — we read the source and eval just the pure
// function out of it, then exercise it behaviourally.
//
// Runs under bare Node: `node static/admin/scripts/routing/__tests__/sidebar-active.test.js`.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');

const routerSrc = fs.readFileSync(
  path.join(__dirname, '..', 'router.js'),
  'utf8'
);
const appSrc = fs.readFileSync(
  path.join(__dirname, '..', '..', 'app.js'),
  'utf8'
);

// Extract the pure `resolveActiveRoute` (2-space-indented function; its body's
// inner braces are deeper-indented, so the first `\n  }` is its close).
const match = routerSrc.match(
  /function resolveActiveRoute\(hashPath, routes\) \{[\s\S]*?\n {2}\}/
);
assert.ok(match, 'router.js defines resolveActiveRoute');
// eslint-disable-next-line no-eval
const resolveActiveRoute = eval('(' + match[0] + ')');

// The real Operations-section routes that collide.
const routes = [
  'ops/accounts',
  'ops/sequencer',
  'ops/sequencer/recovery',
  'ops/system-health',
  'dashboard',
];

// The bug case: on the recovery page, the longest match is recovery itself —
// NOT the shorter `ops/sequencer` sibling.
assert.equal(
  resolveActiveRoute('ops/sequencer/recovery', routes),
  'ops/sequencer/recovery',
  'recovery page lights the recovery item'
);
assert.notEqual(
  resolveActiveRoute('ops/sequencer/recovery', routes),
  'ops/sequencer',
  'recovery page does NOT light the Sequencer item'
);

// The control: on the Sequencer page, only Sequencer matches.
assert.equal(
  resolveActiveRoute('ops/sequencer', routes),
  'ops/sequencer',
  'sequencer page lights the Sequencer item'
);

// Preserved behaviour: a detail page with no nav entry of its own still lights
// its parent (the parent is then the only — hence longest — match).
assert.equal(
  resolveActiveRoute('ops/accounts/did:plc:abc123', routes),
  'ops/accounts',
  'an account-detail page lights its parent Accounts item'
);

// A flat route lights exactly itself.
assert.equal(resolveActiveRoute('dashboard', routes), 'dashboard');

// No match → nothing active (rather than a wrong highlight).
assert.equal(resolveActiveRoute('nowhere/at-all', routes), null);
assert.equal(resolveActiveRoute('ops/sequence', routes), null,
  'a non-boundary prefix does not match (ops/sequence is not ops/sequencer)');

// Both highlight drivers route through the shared resolver (no naive per-item
// startsWith survives in either place).
assert.ok(
  /resolveActiveRoute\(hashPath, routes\)/.test(routerSrc),
  'router updateSidebarActive uses resolveActiveRoute'
);
assert.ok(
  appSrc.includes('AuroraRouter.resolveActiveRoute'),
  'app.js markActive shares the router resolver'
);

console.log('sidebar-active.test.js: all assertions passed');
