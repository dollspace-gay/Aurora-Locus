// Pins the theme-selection consolidation (#441): the redundant per-operator
// Theme dropdown on the "UI & modes" page is removed, and the Installed Themes
// gallery becomes the sole theme surface — a personal "Use this theme" for every
// operator, plus the superadmin deployment-default action beneath it.
//
//   node static/admin/scripts/pages/__tests__/theme-selection-consolidation.test.js

'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('node:assert/strict');
const { test } = require('node:test');

const PAGES = path.resolve(__dirname, '..');
const read = (rel) => fs.readFileSync(path.join(PAGES, rel), 'utf8');

const uiModes = read('ConfigUiModes.js');
const themes = read('ConfigThemes.js');

test('the redundant Theme dropdown card is removed from UI & modes; Language stays', () => {
  assert.ok(!uiModes.includes('ui-theme-toggle'), 'the Theme dropdown mount point must be gone');
  assert.ok(!uiModes.includes('mountDropdown'), 'the ThemeToggle dropdown must not be mounted here');
  assert.ok(uiModes.includes('ui-language'), 'the Language control stays');
  assert.ok(uiModes.includes('installed-themes'), 'the Installed Themes section stays');
});

test('Installed Themes cards offer a personal "Use this theme" for every operator', () => {
  assert.ok(themes.includes('data-usetheme'), 'a per-card personal action exists');
  assert.ok(themes.includes('Use this theme'), 'it is labeled "Use this theme"');
  // The former Theme dropdown's action: pin the personal localStorage theme.
  assert.ok(
    themes.includes('AuroraSettings.setTheme'),
    'the personal action applies the localStorage theme (the former dropdown action)',
  );
  assert.ok(themes.includes('Active Theme'), 'the active personal theme shows an "Active Theme" marker');
});

test('the superadmin deployment-default action is preserved', () => {
  assert.ok(themes.includes('data-setdefault'), 'the set-deployment-default action still exists');
  assert.ok(
    themes.includes('theme.deployment-default'),
    'it still writes the deployment-default runtime setting (audit entry lands)',
  );
  assert.ok(themes.includes('isSuper'), 'it is gated on the superadmin flag');
});
