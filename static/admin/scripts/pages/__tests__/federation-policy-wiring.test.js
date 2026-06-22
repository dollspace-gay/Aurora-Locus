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

test('read-only, manual refresh, future-cycle framing', () => {
  assert.ok(!/setRuntimeSetting\s*\(/.test(src), 'no setRuntimeSetting (read-only)');
  assert.ok(src.includes('fed-refresh') && !/setInterval/.test(src), 'manual refresh, no auto-poll');
  assert.ok(/future cycle/i.test(src) && /restart/i.test(src), 'honest env/restart deferral framing');
  assert.ok(src.includes('#ops/federation'), 'links to Operations → Federation for live status');
});

test('endpoint wrappers exist + the stub row is removed', () => {
  const ep = read('../api/endpoints.js');
  assert.ok(/getFederationPolicy:\s*\(\)\s*=>/.test(ep), 'ops.getFederationPolicy wrapper');
  assert.ok(/describeFederationPosture:\s*\(\)\s*=>/.test(ep), 'atproto.describeFederationPosture wrapper');
  const stubs = read('ConfigStubs.js');
  assert.ok(!/key:\s*'configFederationPolicy'/.test(stubs), 'federation stub row removed');
});
