// Static pins for the Federation policy read-only status page (#344). Mirrors
// the #342/#343 shape: env-config surfaced read-only + the two public describe
// endpoints rendered as the peer-visible posture + honest future-cycle framing.
// Guards no-mutation + the data-source wiring.
//
//   node static/admin/scripts/pages/__tests__/federation-policy-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (name) => fs.readFileSync(path.join(PAGES, name), 'utf8');
const src = read('ConfigFederationPolicy.js');

test('registers its key and reads the SuperAdmin full env view for sections 1-8', () => {
  assert.ok(/register\('configFederationPolicy'/.test(src), 'registers configFederationPolicy');
  assert.ok(src.includes('getFederationPolicy()'), 'reads the SuperAdmin full federation config');
  // surfaces the security-adjacent fields the public describes omit
  assert.ok(/peerPds/.test(src), 'surfaces the trusted peer allowlist');
  assert.ok(/autoStreamEvents/.test(src), 'surfaces the auto-stream toggle');
});

test('peer-visible posture renders the two PUBLIC describe endpoints', () => {
  assert.ok(src.includes('describeServer()'), 'renders describeServer federation block');
  assert.ok(src.includes('describeFederationPosture()'), 'renders describePosture');
});

test('non-peer sections stay read-only env/restart config, manual refresh', () => {
  // Peer mutation uses dedicated ops XRPCs, NOT setRuntimeSetting — the other
  // sections remain read-only env config.
  assert.ok(!/setRuntimeSetting\s*\(/.test(src), 'no setRuntimeSetting (sections stay read-only)');
  assert.ok(src.includes('fed-refresh') && !/setInterval/.test(src), 'manual refresh, no auto-poll');
  assert.ok(/future cycle/i.test(src) && /restart/i.test(src), 'discovery/relay still env/restart deferral framing');
  assert.ok(src.includes('#ops/federation'), 'links to Operations → Federation for live status');
});

// v0.9 Federation Pattern-1 Phase B (#352) — peer-allowlist CRUD wiring.
test('peer CRUD: SuperAdmin-gated affordances call the three ops XRPCs', () => {
  assert.ok(/hasRole\('superadmin'\)/.test(src), 'SuperAdmin-gated client-side');
  assert.ok(/ops\.addFederationPeer\(/.test(src), 'add wired');
  assert.ok(/ops\.removeFederationPeer\(/.test(src), 'remove wired');
  assert.ok(/ops\.modifyFederationPeer\(/.test(src), 'modify wired');
  assert.ok(/destructiveConfirm/.test(src), 'remove uses a destructive confirm');
});

test('peer CRUD: recovery-mode lockout + 4xx-inline / 5xx-toast error split', () => {
  assert.ok(/source === 'RecoveryMode'/.test(src), 'detects recovery via the substrate signal');
  assert.ok(/recoveryActive/.test(src) && /fed-recovery-banner/.test(src), 'greys affordances + banner in recovery');
  assert.ok(/status >= 400 && status < 500/.test(src), 'splits 4xx → inline');
  assert.ok(/AuroraInlineError/.test(src), '4xx errors render inline');
  assert.ok(/AuroraToast\.danger/.test(src), '5xx errors surface as toast');
});

// v0.9 Federation Pattern-1 Phase C (#353) — discovery modes + pending surface.
test('discovery: 3-mode selector + auto-accept threat warning + setDiscoveryMode', () => {
  assert.ok(/fed-discovery-mode/.test(src), 'discovery mode selector present');
  assert.ok(/allowlist-only/.test(src) && /auto-accept/.test(src) && /discovery-disabled/.test(src), 'all 3 modes offered');
  assert.ok(/ops\.setDiscoveryMode\(/.test(src), 'setDiscoveryMode wired');
  assert.ok(/fed-discovery-warning/.test(src) && /toggleAutoAcceptWarning/.test(src), 'auto-accept threat-model warning (inline)');
  assert.ok(/Switch to auto-accept\?/.test(src), 'auto-accept switch confirm modal');
});

test('discovery: pending list accept/dismiss wired + recovery-aware', () => {
  assert.ok(/fed-pending-list/.test(src), 'pending list container');
  assert.ok(/ops\.dismissPendingDiscovery\(/.test(src), 'dismiss wired');
  assert.ok(/acceptPending/.test(src) && /addFederationPeer\(/.test(src), 'accept reuses addFederationPeer');
  assert.ok(/No pending peer discoveries/.test(src), 'empty-state message');
  assert.ok(/modeSel\.disabled = recoveryActive/.test(src), 'mode selector greyed in recovery');
});

test('endpoint wrappers exist + the stub row is removed', () => {
  const ep = read('../api/endpoints.js');
  assert.ok(/getFederationPolicy:\s*\(\)\s*=>/.test(ep), 'ops.getFederationPolicy wrapper');
  assert.ok(/addFederationPeer:\s*\(body\)\s*=>/.test(ep), 'ops.addFederationPeer wrapper');
  assert.ok(/removeFederationPeer:\s*\(body\)\s*=>/.test(ep), 'ops.removeFederationPeer wrapper');
  assert.ok(/modifyFederationPeer:\s*\(body\)\s*=>/.test(ep), 'ops.modifyFederationPeer wrapper');
  assert.ok(/setDiscoveryMode:\s*\(body\)\s*=>/.test(ep), 'ops.setDiscoveryMode wrapper');
  assert.ok(/dismissPendingDiscovery:\s*\(body\)\s*=>/.test(ep), 'ops.dismissPendingDiscovery wrapper');
  assert.ok(/describeFederationPosture:\s*\(\)\s*=>/.test(ep), 'atproto.describeFederationPosture wrapper');
  const stubs = read('ConfigStubs.js');
  assert.ok(!/key:\s*'configFederationPolicy'/.test(stubs), 'federation stub row removed');
});
