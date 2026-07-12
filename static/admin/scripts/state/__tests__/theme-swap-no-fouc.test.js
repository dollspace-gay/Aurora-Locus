// Pins the gap-free theme swap (#441 live-swap FOUC). Clicking "Use this theme"
// applies via AuroraSettings.applyTheme; re-pointing the existing <link> href
// dropped the old CSS during the (no-store) refetch, flashing the base defaults.
// applyTheme must instead load a fresh <link> and only remove the old once the
// new has loaded.
//
//   node static/admin/scripts/state/__tests__/theme-swap-no-fouc.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const src = fs.readFileSync(path.resolve(__dirname, '..', 'settings.js'), 'utf8');

test('applyTheme swaps theme stylesheets gap-free (load fresh <link>, then drop old)', () => {
  assert.ok(src.includes('swapThemeLink'), 'applyTheme delegates to a gap-free swap helper');
  assert.ok(src.includes("createElement('link')"), 'a fresh <link> is created for the new theme');
  assert.ok(
    src.includes("addEventListener('load'"),
    'the old link is removed on the new one\'s load, not before',
  );
  // The old FOUC pattern — re-pointing the existing links\' href directly — must
  // be gone from applyTheme.
  assert.ok(
    !src.includes("tokensLink.setAttribute('href'"),
    'applyTheme must not re-point the existing link href (the swap-gap source)',
  );
});
