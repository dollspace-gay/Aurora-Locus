// i18n string helper (substrate primitive 16) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.16.
//
// API:
//   t('queue.title')                      → "Queue"
//   t('reports.count', { count: 0 })      → "No reports"
//   t('reports.count', { count: 1 })      → "1 report"
//   t('reports.count', { count: 5 })      → "5 reports"
//   t('common.error', { message: 'foo' }) → "Error: foo"
//
// Locale loading: fetches /admin/i18n/<locale>.json on init based
// on operator preference (Settings → UI & modes language selector,
// defaults to navigator.language). Falls back to English if the
// requested locale's file doesn't exist.
//
// ICU MessageFormat: minimal subset implemented inline — supports
// {key} substitution and {key, plural, =N {…} one {…} other {…}}
// patterns. Wider ICU (select, ordinals, gender, nested) deferred
// to v0.3 if a non-English locale needs them.

(function (global) {
  'use strict';

  let strings = {};
  let activeLocale = 'en';

  function get(obj, dotPath) {
    const parts = dotPath.split('.');
    let cur = obj;
    for (const p of parts) {
      if (cur && typeof cur === 'object' && p in cur) {
        cur = cur[p];
      } else {
        return undefined;
      }
    }
    return cur;
  }

  // Resolve a single template by substituting {placeholders}.
  // Honors {key, plural, =N {…} one {…} other {…}} for the simplest
  // ICU plural form. Manual brace-aware parser so nested arm bodies
  // ({# reports}) don't trip the regex.
  function format(template, params) {
    if (typeof template !== 'string') return template;
    if (!params) params = {};
    let out = '';
    let i = 0;
    while (i < template.length) {
      if (template[i] !== '{') {
        out += template[i++];
        continue;
      }
      // Find the matching close brace, balanced.
      let depth = 1;
      let j = i + 1;
      while (j < template.length && depth > 0) {
        if (template[j] === '{') depth++;
        else if (template[j] === '}') depth--;
        if (depth === 0) break;
        j++;
      }
      if (depth !== 0) {
        // Unbalanced — emit the rest as-is.
        out += template.substring(i);
        break;
      }
      const inside = template.substring(i + 1, j);
      // Plural form: "key, plural, =0 {…} one {…} other {…}"
      const pluralMatch = inside.match(/^(\w+)\s*,\s*plural\s*,\s*([\s\S]+)$/);
      if (pluralMatch) {
        out += resolvePlural(params[pluralMatch[1]], pluralMatch[2]);
      } else if (/^\w+$/.test(inside)) {
        const v = params[inside];
        out += v == null ? '' : String(v);
      } else {
        // Unknown form — emit verbatim.
        out += '{' + inside + '}';
      }
      i = j + 1;
    }
    return out;
  }

  function resolvePlural(count, body) {
    if (typeof count !== 'number' && typeof count !== 'string') {
      // Fall back to "other" arm if no count.
      const other = matchArm(body, 'other');
      if (other != null) return other.replace(/#/g, '?');
      return '';
    }
    const n = Number(count);
    // Try =N first, then one (when n === 1), then other.
    const exact = matchArm(body, '=' + n);
    if (exact != null) return exact.replace(/#/g, String(n));
    if (n === 1) {
      const one = matchArm(body, 'one');
      if (one != null) return one.replace(/#/g, String(n));
    }
    const other = matchArm(body, 'other');
    if (other != null) return other.replace(/#/g, String(n));
    return String(n);
  }

  // Extract the body of `selector { … }` from the plural body string.
  // Handles balanced braces inside the body.
  function matchArm(body, selector) {
    const idx = body.indexOf(selector);
    if (idx < 0) return null;
    // Find the opening brace after the selector.
    let i = idx + selector.length;
    while (i < body.length && body[i] !== '{') i++;
    if (i >= body.length) return null;
    let depth = 1;
    let start = ++i;
    while (i < body.length && depth > 0) {
      if (body[i] === '{') depth++;
      else if (body[i] === '}') depth--;
      if (depth === 0) break;
      i++;
    }
    if (depth !== 0) return null;
    return body.substring(start, i);
  }

  function t(key, params) {
    const template = get(strings, key);
    if (template == null) return key;
    return format(template, params);
  }

  async function loadLocale(locale) {
    try {
      const res = await fetch(`/admin/i18n/${locale}.json`);
      if (!res.ok) throw new Error('locale fetch failed');
      strings = await res.json();
      activeLocale = locale;
    } catch (e) {
      // Fall back to English silently.
      if (locale !== 'en') {
        return loadLocale('en');
      }
      // Bare fallback: empty map; t() returns keys unchanged so the
      // UI still renders something legible.
      strings = {};
    }
  }

  global.AuroraI18n = {
    t: t,
    load: loadLocale,
    locale: () => activeLocale,
    // Exposed for tests.
    _format: format,
    _resolvePlural: resolvePlural,
  };

  // Make t() available as a bare global since it's used everywhere.
  global.t = t;
})(window);
