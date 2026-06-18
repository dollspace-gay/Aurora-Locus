// Contract pins for the bulk repository-repair surface (Arc H §7.4.3 / #293):
// the seven scan/repair XRPC wrappers, the page's consumption of them, the
// typed-confirm gate, route + sidebar wiring, and i18n-key completeness.
//
// Static source reads (the page pulls in DOM / router / modal / polling that's
// heavyweight to mock); the literal facts pinned here are what a regex over the
// source captures at the right granularity. Mirrors repo-rebuild.test.js.
//
// No framework dependency — runs under bare Node via `node
// static/admin/scripts/api/__tests__/repo-repair.test.js`.

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..', '..'); // static/admin
const read = (rel) => fs.readFileSync(path.join(ROOT, rel), 'utf8');

const ENDPOINTS = read('scripts/api/endpoints.js');
const PAGE = read('scripts/pages/RepoRepair.js');
const ROUTES = read('scripts/routing/routes.js');
const EN = JSON.parse(read('i18n/en.json'));

const METHODS = [
  ['scanReposForInconsistencies', 'post'],
  ['getScanProgress', 'get'],
  ['cancelScan', 'post'],
  ['getRepoScanResults', 'get'],
  ['repairRepos', 'post'],
  ['getBulkRepairProgress', 'get'],
  ['cancelBulkRepair', 'post'],
];

test('endpoints.js registers the seven scan/repair XRPC wrappers with correct NSIDs', () => {
  for (const [m, verb] of METHODS) {
    const re = new RegExp(m + ':[\\s\\S]*?C\\(\\)\\.' + verb + "\\('tools\\.aurora\\.superadmin\\." + m + "'");
    assert.match(ENDPOINTS, re, `${m} wrapper (${verb}) missing or wrong NSID`);
  }
});

test('the repair page consumes all seven endpoints', () => {
  for (const [m] of METHODS) {
    assert.match(PAGE, new RegExp('EP\\(\\)\\.' + m + '\\('), `page must call ${m}`);
  }
});

test('bulk repair uses the typed-confirm REPAIR gate + a rationale', () => {
  assert.match(PAGE, /typedConfirmGate:\s*'REPAIR'/, 'typed-confirm gate must be REPAIR');
  assert.match(PAGE, /rationaleRequired:\s*true/);
  // Both repair-all and repair-selected flow the rationale into repairRepos.
  assert.match(PAGE, /all:\s*true,\s*rationale:/);
  assert.match(PAGE, /dids:\s*Array\.from\(selected\),\s*rationale:/);
});

test('the page registers the opsRepoRepair route handler', () => {
  assert.match(PAGE, /AuroraRouter\.register\('opsRepoRepair'/);
});

test('routes.js wires the repair route (SuperAdmin) + a sidebar entry', () => {
  assert.match(ROUTES, /pattern:\s*'ops\/repo-repair',\s*page:\s*'opsRepoRepair',\s*requires:\s*'superadmin'/);
  assert.match(ROUTES, /label:\s*'Repository repair',\s*route:\s*'ops\/repo-repair'[^}]*requires:\s*'superadmin'/);
});

test('every repair.* i18n key the page references exists in en.json', () => {
  const keys = new Set();
  const re = /t\('repair\.([a-z_]+)'/g;
  let m;
  while ((m = re.exec(PAGE)) !== null) keys.add(m[1]);
  // Drop the `severity_` artifact from t('repair.severity_' + sev); the concrete
  // keys are added explicitly.
  keys.delete('severity_');
  for (const s of ['high', 'medium', 'low']) keys.add('severity_' + s);
  assert.ok(keys.size >= 25, 'expected the page to reference many repair.* keys');
  const bucket = EN.repair || {};
  const missing = [...keys].filter((k) => !(k in bucket));
  assert.deepEqual(missing, [], 'en.json is missing repair.* keys: ' + missing.join(', '));
});
