// Static pins for the Dashboard account-growth block (#361). The v0.9
// rewrite already retired the synthetic user-growth chart; #361 adds a REAL
// account-growth sparkline off `actor.created_at`, served by the
// tools.aurora.admin.getAccountGrowth XRPC. One fetch carries both newAccounts
// and cumulativeAccounts per point; a header toggle (default per-day, not
// persisted) picks which the CSS-bar sparkline renders. These guard the
// endpoint wiring, the admin-scope gate, the toggle field selection, the
// no-persistence reset, the i18n keys, and the Chart.js dep removal.
//
//   node static/admin/scripts/pages/__tests__/dashboard-account-growth-wiring.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const SCRIPTS = path.resolve(__dirname, '..', '..');
const dashboard = fs.readFileSync(path.resolve(SCRIPTS, 'pages', 'Dashboard.js'), 'utf8');
const endpoints = fs.readFileSync(path.resolve(SCRIPTS, 'api', 'endpoints.js'), 'utf8');
const en = JSON.parse(
  fs.readFileSync(path.resolve(SCRIPTS, '..', 'i18n', 'en.json'), 'utf8'),
);
const indexHtml = fs.readFileSync(path.resolve(SCRIPTS, '..', 'index.html'), 'utf8');
const pagesCss = fs.readFileSync(path.resolve(SCRIPTS, '..', 'styles', 'pages.css'), 'utf8');

test('endpoints registers getAccountGrowth against the admin NSID', () => {
  assert.ok(
    /getAccountGrowth:\s*\(\)\s*=>\s*C\(\)\.get\('tools\.aurora\.admin\.getAccountGrowth'\)/.test(endpoints),
    'getAccountGrowth GETs tools.aurora.admin.getAccountGrowth with no params',
  );
});

test('the block exists and is admin-scoped (not Moderator+)', () => {
  assert.ok(dashboard.includes("id: 'accountgrowth'"), 'account-growth block declared');
  // Visibility mirrors the deployment-overview block: Admin+ and not disabled.
  assert.ok(
    /id:\s*'accountgrowth',\s*\n\s*visible:\s*\(c\)\s*=>\s*c\.isAdmin\s*&&\s*c\.notDisabled/.test(dashboard),
    'block gated on c.isAdmin && c.notDisabled',
  );
});

test('the block fetches from the admin endpoint', () => {
  assert.ok(dashboard.includes('ep.admin.getAccountGrowth()'), 'refresh calls getAccountGrowth');
});

test('default mode is per-day and is reset on mount (no persistence)', () => {
  assert.ok(/let growthMode = 'perDay'/.test(dashboard), 'default toggle state is perDay');
  assert.ok(
    /growthMode = 'perDay';\s*\n\s*growthData = null;/.test(dashboard),
    'mount resets the toggle + cache (no cross-load persistence)',
  );
});

test('the toggle picks cumulative vs per-day fields and re-renders without re-fetch', () => {
  // renderGrowth selects the series field by mode; the toggle handler calls
  // renderGrowth (not refresh) so toggling never re-hits the network.
  assert.ok(
    dashboard.includes("cumulative ? 'cumulativeAccounts' : 'newAccounts'"),
    'field selection keys off the cumulative toggle',
  );
  assert.ok(
    /addEventListener\('change',\s*\(\)\s*=>\s*\{\s*\n\s*growthMode = sel\.value;\s*\n\s*renderGrowth\(\)/.test(dashboard),
    'toggle re-renders from cache, no re-fetch',
  );
});

test('renders with the shared CSS-bar sparkline idiom (no chart dep)', () => {
  assert.ok(dashboard.includes("'<span class=\"spark-bar"), 'uses spark-bar bars');
  assert.ok(dashboard.includes("sparkline sparkline-tall"), 'uses the sparkline container');
  // And the CSS that makes inline bars actually render exists.
  assert.ok(/\.spark-bar\s*\{/.test(pagesCss), 'pages.css defines .spark-bar');
  assert.ok(/\.sparkline\s*\{/.test(pagesCss), 'pages.css defines .sparkline');
});

test('i18n keys the block reads exist', () => {
  for (const k of [
    'growth_title', 'growth_mode', 'growth_per_day', 'growth_cumulative',
    'growth_none', 'growth_new_in_window', 'growth_total_accounts', 'growth_window',
  ]) {
    assert.ok(en.dashboard && en.dashboard[k], 'en.json has dashboard.' + k);
  }
});

test('the dead Chart.js CDN script is gone (Chart.js cleanup fold)', () => {
  assert.ok(!/chart\.js/i.test(indexHtml), 'index.html no longer loads Chart.js');
  assert.ok(!/cdn\.jsdelivr/i.test(indexHtml), 'index.html has no CDN script left');
});
