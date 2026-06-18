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
