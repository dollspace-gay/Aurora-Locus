// Locale-aware date / number / duration / relative-time formatters.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §11.4: every user-facing date and
// number routes through Intl with the active locale. Hardcoded format
// strings (toLocaleString() without args, Date.prototype.toString())
// are forbidden in component code.

(function (global) {
  'use strict';

  function activeLocale() {
    if (global.AuroraI18n && typeof global.AuroraI18n.locale === 'function') {
      return global.AuroraI18n.locale();
    }
    return 'en';
  }

  function formatDate(value, format) {
    if (value == null) return '';
    const d = (value instanceof Date) ? value : new Date(value);
    if (isNaN(d.getTime())) return '';
    let opts;
    switch (format) {
      case 'short': opts = { dateStyle: 'short' }; break;
      case 'medium': opts = { dateStyle: 'medium' }; break;
      case 'long': opts = { dateStyle: 'long' }; break;
      case 'datetime':
      default: opts = { dateStyle: 'short', timeStyle: 'short' };
    }
    try {
      return new Intl.DateTimeFormat(activeLocale(), opts).format(d);
    } catch (e) {
      return d.toISOString();
    }
  }

  function formatNumber(n, options) {
    if (n == null || isNaN(Number(n))) return '';
    try {
      return new Intl.NumberFormat(activeLocale(), options || {}).format(Number(n));
    } catch (e) {
      return String(n);
    }
  }

  function formatRelativeTime(value) {
    if (value == null) return '';
    const d = (value instanceof Date) ? value : new Date(value);
    if (isNaN(d.getTime())) return '';
    const seconds = Math.floor((Date.now() - d.getTime()) / 1000);
    let amount, unit;
    if (Math.abs(seconds) < 60) { amount = -seconds; unit = 'second'; }
    else if (Math.abs(seconds) < 3600) { amount = -Math.floor(seconds / 60); unit = 'minute'; }
    else if (Math.abs(seconds) < 86400) { amount = -Math.floor(seconds / 3600); unit = 'hour'; }
    else if (Math.abs(seconds) < 2592000) { amount = -Math.floor(seconds / 86400); unit = 'day'; }
    else if (Math.abs(seconds) < 31536000) { amount = -Math.floor(seconds / 2592000); unit = 'month'; }
    else { amount = -Math.floor(seconds / 31536000); unit = 'year'; }
    try {
      return new Intl.RelativeTimeFormat(activeLocale(), { numeric: 'auto' }).format(amount, unit);
    } catch (e) {
      return formatDate(d, 'short');
    }
  }

  // Compact "2h" / "5d" duration label for stat cards. Locale-aware
  // unit narrow form would require Intl.DurationFormat which is not
  // universally supported yet; fall back to ASCII suffix.
  function formatDurationCompact(seconds) {
    if (seconds == null || seconds === 0) return '—';
    const abs = Math.abs(seconds);
    if (abs < 60) return Math.round(abs) + 's';
    if (abs < 3600) return Math.round(abs / 60) + 'm';
    if (abs < 86400) return Math.round(abs / 3600) + 'h';
    return Math.round(abs / 86400) + 'd';
  }

  function formatBytes(bytes) {
    if (bytes == null || isNaN(Number(bytes))) return '';
    const n = Number(bytes);
    if (n < 1024) return formatNumber(n) + ' B';
    if (n < 1024 * 1024) return formatNumber(n / 1024, { maximumFractionDigits: 1 }) + ' KB';
    if (n < 1024 * 1024 * 1024) return formatNumber(n / 1024 / 1024, { maximumFractionDigits: 1 }) + ' MB';
    return formatNumber(n / 1024 / 1024 / 1024, { maximumFractionDigits: 2 }) + ' GB';
  }

  // Determine the locale's first day of week (0 = Sunday). Used by
  // calendar widget per §6.20. Intl.Locale.weekInfo is the Intl-native
  // path; fall back to Sunday for unsupported environments.
  function firstDayOfWeek() {
    try {
      const loc = new Intl.Locale(activeLocale());
      // Intl.Locale.weekInfo returns 1 (Monday) … 7 (Sunday).
      const info = loc.weekInfo || (typeof loc.getWeekInfo === 'function' ? loc.getWeekInfo() : null);
      if (info && typeof info.firstDay === 'number') {
        return info.firstDay === 7 ? 0 : info.firstDay;
      }
    } catch (e) { /* fallthrough */ }
    return activeLocale().startsWith('en-US') ? 0 : 1;
  }

  // Locale-aware list joiner (§10.3.4 / §16 D3) — "A, B, and C" / "… or C".
  // Wraps Intl.ListFormat with an English comma+conjunction fallback so a
  // missing-API environment still reads naturally.
  function formatList(items, type) {
    const arr = Array.isArray(items) ? items.map((x) => String(x == null ? '' : x)) : [];
    try {
      return new Intl.ListFormat(activeLocale(), { style: 'long', type: type || 'conjunction' }).format(arr);
    } catch (e) {
      if (arr.length <= 1) return arr.join('');
      const conj = (type === 'disjunction') ? 'or' : 'and';
      return arr.slice(0, -1).join(', ') + (arr.length > 2 ? ',' : '') + ' ' + conj + ' ' + arr[arr.length - 1];
    }
  }

  global.AuroraFormat = {
    date: formatDate,
    number: formatNumber,
    relativeTime: formatRelativeTime,
    durationCompact: formatDurationCompact,
    bytes: formatBytes,
    list: formatList,
    firstDayOfWeek: firstDayOfWeek,
  };
})(window);
