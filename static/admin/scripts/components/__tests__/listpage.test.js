// Static-source pins for the shared list-page helpers (#257).
//
// AuroraListPage (components/ListPage.js) holds the filter-state,
// cursor-pagination, and mod-events subscription routines that were
// byte-identical across the moderation list pages (Appeals/Audit/Events/
// Reports). These pins lock:
//
//   1. The helper module exposes the four-function API the pages consume.
//   2. index.html loads the module before the pages that use it.
//   3. (added with the page refactor) each page composes the helpers and no
//      longer carries the duplicated machinery inline.
//
// No framework dependency — runs under bare Node:
//   node static/admin/scripts/components/__tests__/listpage.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const COMPONENTS_DIR = path.resolve(__dirname, '..');
const ADMIN_DIR = path.resolve(__dirname, '..', '..', '..');

function readComponent(name) {
  return fs.readFileSync(path.join(COMPONENTS_DIR, name), 'utf8');
}

test('ListPage.js exposes the AuroraListPage helper API', () => {
  const src = readComponent('ListPage.js');
  assert.match(src, /global\.AuroraListPage\s*=/, 'attaches AuroraListPage to the global');
  for (const fn of ['readFilters', 'applyFilters', 'renderPagination', 'subscribeModEvents']) {
    assert.match(
      src,
      new RegExp('function\\s+' + fn + '\\s*\\('),
      `ListPage.js must define ${fn}`,
    );
    assert.match(
      src,
      new RegExp(fn + '\\s*:\\s*' + fn),
      `ListPage.js must export ${fn} on AuroraListPage`,
    );
  }
});

test('ListPage.js stays cursor-pagination-only (mutates the page-owned stack in place)', () => {
  const src = readComponent('ListPage.js');
  // The helper pops/pushes the page's cursorStack array; it never reassigns a
  // module-level stack (state lives in the page, not the helper).
  assert.match(src, /cursorStack\.pop\(\)/, 'pops the cursor stack');
  assert.match(src, /cursorStack\.push\(/, 'pushes the next cursor');
});

test('index.html loads ListPage.js before the list pages that consume it', () => {
  const html = fs.readFileSync(path.join(ADMIN_DIR, 'index.html'), 'utf8');
  const listPageIdx = html.indexOf('components/ListPage.js');
  assert.ok(listPageIdx !== -1, 'index.html must include components/ListPage.js');
  for (const page of ['Appeals.js', 'Audit.js', 'Events.js', 'Reports.js']) {
    const pageIdx = html.indexOf('pages/' + page);
    assert.ok(pageIdx !== -1, `index.html must include pages/${page}`);
    assert.ok(
      listPageIdx < pageIdx,
      `ListPage.js must load before pages/${page} (it is consumed at mount time)`,
    );
  }
});

// ---- per-page extraction pins (added with the commit-2 refactor) ----
//
// Each moderation list page must COMPOSE the helpers (positive proof) and must
// no longer carry the duplicated machinery inline (negative proof — the body
// of readFilters / renderPagination, and the direct UrlState / Subscription
// calls, moved into AuroraListPage). The page still declares SCALAR_KEYS /
// BOOL_KEYS and keeps its own cursorStack/lastFilters state + thin
// applyFilters/renderPagination wrappers — those are not pinned away.

const PAGES_DIR = path.join(COMPONENTS_DIR, '..', 'pages');
function readPage(name) {
  return fs.readFileSync(path.join(PAGES_DIR, name), 'utf8');
}

for (const page of ['Appeals.js', 'Audit.js', 'Events.js', 'Reports.js']) {
  test(`${page} composes the AuroraListPage filter + pagination helpers`, () => {
    const src = readPage(page);
    assert.match(src, /AuroraListPage\.readFilters\(/, `${page} must use AuroraListPage.readFilters`);
    assert.match(src, /AuroraListPage\.applyFilters\(/, `${page} must use AuroraListPage.applyFilters`);
    assert.match(src, /AuroraListPage\.renderPagination\(/, `${page} must use AuroraListPage.renderPagination`);
  });

  test(`${page} no longer inlines the extracted machinery`, () => {
    const src = readPage(page);
    // The cursor-stack mutation and the URL-state round-trip moved into the
    // helper; a page that still inlines them has not been deduplicated.
    assert.doesNotMatch(src, /cursorStack\.pop\(/, `${page} must not inline cursor-stack pop (moved to helper)`);
    assert.doesNotMatch(src, /AuroraUrlState\.read\(/, `${page} must not call AuroraUrlState.read directly (moved to helper)`);
    assert.doesNotMatch(src, /AuroraUrlState\.write\(/, `${page} must not call AuroraUrlState.write directly (moved to helper)`);
  });
}

for (const page of ['Audit.js', 'Events.js']) {
  test(`${page} composes the shared mod-events subscription helper`, () => {
    const src = readPage(page);
    assert.match(src, /AuroraListPage\.subscribeModEvents\(/, `${page} must use AuroraListPage.subscribeModEvents`);
    assert.doesNotMatch(src, /AuroraSubscription\.subscribe\(/, `${page} must not call AuroraSubscription.subscribe directly (moved to helper)`);
  });
}
