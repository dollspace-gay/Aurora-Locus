// Contract pins for the repository-rebuild surface (Arc H §7.4.1 / #288):
// the four superadmin XRPC wrappers, the page's consumption of them, the
// route + sidebar wiring, and the i18n-key completeness of the page.
//
// Static source reads rather than running-page tests — the page pulls in DOM,
// AuroraRouter, AuroraModal, and polling infrastructure that's heavyweight to
// mock; the contracts worth pinning (endpoint NSIDs, the typed-confirm gate,
// route registration, and no-missing-i18n-keys) are all literal facts a regex
// over the source captures at the right granularity. Mirrors the sibling
// endpoints.test.js convention.
//
// No framework dependency — runs under bare Node via `node
// static/admin/scripts/api/__tests__/repo-rebuild.test.js`.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..', '..'); // static/admin
const read = (rel) => fs.readFileSync(path.join(ROOT, rel), 'utf8');

const ENDPOINTS = read('scripts/api/endpoints.js');
const PAGE = read('scripts/pages/RepoRebuild.js');
const ROUTES = read('scripts/routing/routes.js');
const EN = JSON.parse(read('i18n/en.json'));

test('endpoints.js registers the four rebuild XRPC wrappers with correct NSIDs', () => {
  assert.match(ENDPOINTS, /preRebuildCheck:\s*\(params\)\s*=>\s*C\(\)\.get\('tools\.aurora\.superadmin\.preRebuildCheck'/);
  assert.match(ENDPOINTS, /rebuildRepo:\s*\(body\)\s*=>\s*C\(\)\.post\('tools\.aurora\.superadmin\.rebuildRepo'/);
  assert.match(ENDPOINTS, /getRebuildProgress:[\s\S]*?C\(\)\.get\('tools\.aurora\.superadmin\.getRebuildProgress'/);
  assert.match(ENDPOINTS, /cancelRebuild:[\s\S]*?C\(\)\.post\('tools\.aurora\.superadmin\.cancelRebuild'/);
});

test('the rebuild page consumes all four endpoints', () => {
  for (const m of ['preRebuildCheck', 'rebuildRepo', 'getRebuildProgress', 'cancelRebuild']) {
    assert.match(PAGE, new RegExp('EP\\(\\)\\.' + m + '\\('), `page must call ${m}`);
  }
});

test('the destructive rebuild uses the typed-confirm REBUILD gate + a rationale', () => {
  assert.match(PAGE, /typedConfirmGate:\s*'REBUILD'/, 'typed-confirm gate must be REBUILD');
  assert.match(PAGE, /rationaleRequired:\s*true/, 'rebuild confirm must require a rationale');
  // The rationale from the confirm flows into the rebuildRepo body.
  assert.match(PAGE, /rebuildRepo\(\{\s*did:\s*currentDid,\s*rationale:/);
});

test('the page registers the opsRepoRebuild route handler', () => {
  assert.match(PAGE, /AuroraRouter\.register\('opsRepoRebuild'/);
});

test('routes.js wires the rebuild routes (SuperAdmin) + a sidebar entry', () => {
  assert.match(ROUTES, /pattern:\s*'ops\/repo-rebuild',\s*page:\s*'opsRepoRebuild',\s*requires:\s*'superadmin'/);
  assert.match(ROUTES, /pattern:\s*'ops\/repo-rebuild\/:did',\s*page:\s*'opsRepoRebuild',\s*requires:\s*'superadmin'/);
  assert.match(ROUTES, /label:\s*'Repository rebuild',\s*route:\s*'ops\/repo-rebuild'[^}]*requires:\s*'superadmin'/);
});

test('deep preflight renders the signing-key rotation count (#367)', () => {
  // Reads the keyHistory fields the A1 preRebuildCheck deep mode adds.
  assert.match(PAGE, /r\.rotatedKeysCount/, 'page must render rotatedKeysCount');
  assert.match(PAGE, /r\.keyHistoryError/, 'page must handle keyHistoryError fallback');
  assert.match(PAGE, /t\('rebuild\.key_rotations'\)/, 'page must use the key_rotations label');
});

test('deep preflight renders history-aware verify result (#368)', () => {
  assert.match(PAGE, /r\.historyAwareVerifyResult/, 'page must read historyAwareVerifyResult');
  assert.match(PAGE, /hv\.verified === true/, 'page must render the verified verdict');
  assert.match(PAGE, /hv\.verified === false/, 'page must render the failed verdict');
  assert.match(PAGE, /r\.historyAwareVerifyError/, 'page must handle the can\'t-run error');
  assert.match(PAGE, /t\('rebuild\.history_verify_ok'/, 'page must use the history_verify_ok label');
});

test('every rebuild.* i18n key the page references exists in en.json', () => {
  // Literal t('rebuild.<key>') references.
  const keys = new Set();
  const re = /t\('rebuild\.([a-z_]+)'/g;
  let m;
  while ((m = re.exec(PAGE)) !== null) keys.add(m[1]);
  // Drop the `phase_` artifact captured from the dynamic concatenation
  // t('rebuild.phase_' + phase); the concrete phase keys are added below.
  keys.delete('phase_');
  // Dynamically-built phase keys: t('rebuild.phase_' + phase).
  for (const ph of ['pending', 'walking', 'verifying', 'swapping', 'completed', 'failed', 'cancelled']) {
    keys.add('phase_' + ph);
  }
  assert.ok(keys.size >= 20, 'expected the page to reference many rebuild.* keys');
  const bucket = EN.rebuild || {};
  const missing = [...keys].filter((k) => !(k in bucket));
  assert.deepEqual(missing, [], 'en.json is missing rebuild.* keys: ' + missing.join(', '));
});
