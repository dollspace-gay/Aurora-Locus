// i18n substrate tests (substrate primitive 16, §13.2 Layer 1).
//
// Covers: bare key resolution, parameter substitution, plural form
// (=N exact / one / other), missing key fallback (returns key),
// missing param fallback (substitutes empty string).

'use strict';

const fs = require('fs');
const path = require('path');
const { test } = require('node:test');
const assert = require('node:assert/strict');

function loadI18n() {
  const stub = { fetch: () => Promise.reject(new Error('not stubbed')) };
  globalThis.fetch = stub.fetch;
  const src = fs.readFileSync(
    path.resolve(__dirname, '..', 'i18n.js'),
    'utf8',
  );
  const fn = new Function(
    'window',
    src + '\nreturn { AuroraI18n: window.AuroraI18n, t: window.t };',
  );
  return fn(stub);
}

function withStrings(strings) {
  const { AuroraI18n, t } = loadI18n();
  // Inject a test string set by replicating what loadLocale would do.
  // Since strings is a closure-private, we re-evaluate the source
  // with a pre-loaded fetch stub instead.
  const stub = {
    fetch: () =>
      Promise.resolve({
        ok: true,
        json: () => Promise.resolve(strings),
      }),
  };
  globalThis.fetch = stub.fetch;
  const src = fs.readFileSync(
    path.resolve(__dirname, '..', 'i18n.js'),
    'utf8',
  );
  const fn = new Function(
    'window',
    src + '\nreturn { AuroraI18n: window.AuroraI18n, t: window.t };',
  );
  return fn(stub);
}

test('returns key unchanged when not found', async () => {
  const { t } = withStrings({ queue: { title: 'Queue' } });
  assert.equal(t('not.a.real.key'), 'not.a.real.key');
});

test('resolves nested keys', async () => {
  const { AuroraI18n, t } = withStrings({ queue: { title: 'Queue' } });
  await AuroraI18n.load('en'); // loads via stubbed fetch
  assert.equal(t('queue.title'), 'Queue');
});

test('substitutes {placeholder} parameters', async () => {
  const { AuroraI18n, t } = withStrings({
    common: { error: 'Error: {message}' },
  });
  await AuroraI18n.load('en');
  assert.equal(t('common.error', { message: 'connection refused' }), 'Error: connection refused');
});

test('plural =0 / one / other arms resolve correctly', async () => {
  const { AuroraI18n, t } = withStrings({
    reports: {
      count: '{count, plural, =0 {No reports} one {# report} other {# reports}}',
    },
  });
  await AuroraI18n.load('en');
  assert.equal(t('reports.count', { count: 0 }), 'No reports');
  assert.equal(t('reports.count', { count: 1 }), '1 report');
  assert.equal(t('reports.count', { count: 5 }), '5 reports');
});

test('missing param substitutes empty string', async () => {
  const { AuroraI18n, t } = withStrings({
    common: { error: 'Error: {message}' },
  });
  await AuroraI18n.load('en');
  assert.equal(t('common.error', {}), 'Error: ');
});

test('falls back to English when requested locale fetch fails', async () => {
  let callCount = 0;
  const localeStrings = { greeting: 'Hello!' };
  globalThis.fetch = (url) => {
    callCount++;
    if (url.includes('/fr.json')) {
      return Promise.resolve({ ok: false });
    }
    return Promise.resolve({ ok: true, json: () => Promise.resolve(localeStrings) });
  };
  const src = fs.readFileSync(
    path.resolve(__dirname, '..', 'i18n.js'),
    'utf8',
  );
  const fn = new Function('window', src + '\nreturn window.AuroraI18n;');
  const I18n = fn({ fetch: globalThis.fetch });
  await I18n.load('fr');
  assert.equal(I18n.locale(), 'en', 'should fall back to en after fr fails');
  assert.equal(I18n.t('greeting'), 'Hello!');
  assert.ok(callCount >= 2, 'should attempt fr then fallback to en');
});
