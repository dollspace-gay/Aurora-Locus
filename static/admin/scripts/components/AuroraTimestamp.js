// AuroraTimestamp — the canonical time-display primitive (§10.4.3).
//
// One component, three contexts, so every timestamp across the admin UI
// renders consistently and is locale-aware via Intl (through AuroraFormat,
// which wraps Intl.DateTimeFormat / Intl.RelativeTimeFormat). Replaces the
// ad-hoc per-page formatters (raw `new Date().toLocaleString()`, hand-rolled
// "Xm ago", etc.) the §10.4.3 audit catalogs.
//
//   AuroraTimestamp.render({ value, context })  →  HTML string (a <time> element)
//
// Contexts (per the §10.4.3 convention):
//   - "forensic"  — audit-chain / forensic / copy-paste surfaces: ISO 8601
//                   with timezone as the display; hover (title) shows local +
//                   relative.
//   - "activity"  — activity feeds / recent-events lists: relative time as the
//                   display ("3 minutes ago"); hover shows ISO 8601.
//   - "detail"    — detail pages / settings / precise points-in-time: ISO 8601
//                   with the local-formatted time in parentheses; hover shows
//                   relative. (Default when context is omitted.)
//
// The rendered <time datetime="<iso>"> carries the machine-readable ISO value
// regardless of context, so the display text can vary while the semantic
// timestamp stays copy-paste / scrape friendly.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  function toDate(value) {
    if (value == null) return null;
    const d = value instanceof Date ? value : new Date(value);
    return isNaN(d.getTime()) ? null : d;
  }

  // ISO 8601 with timezone, millisecond-trimmed for readability
  // (2026-06-13T11:30:00Z, not 2026-06-13T11:30:00.000Z).
  function isoString(d) {
    return d.toISOString().replace(/\.\d{3}Z$/, 'Z');
  }

  // Local, locale-aware display via the shared Intl wrapper; degrades to the
  // platform locale string if AuroraFormat hasn't loaded.
  function localString(d) {
    return global.AuroraFormat
      ? global.AuroraFormat.date(d, 'datetime')
      : d.toLocaleString();
  }

  function relativeString(d) {
    return global.AuroraFormat ? global.AuroraFormat.relativeTime(d) : '';
  }

  function render(opts) {
    opts = opts || {};
    const d = toDate(opts.value);
    if (!d) {
      return '<span class="timestamp timestamp-empty">—</span>';
    }
    const context = opts.context || 'detail';
    const iso = isoString(d);
    const local = localString(d);
    const rel = relativeString(d);

    let display;
    let title;
    if (context === 'forensic') {
      display = iso;
      title = local + (rel ? ' · ' + rel : '');
    } else if (context === 'activity') {
      display = rel || local;
      title = iso;
    } else {
      // detail (default)
      display = local ? iso + ' (' + local + ')' : iso;
      title = rel || iso;
    }

    return (
      '<time class="timestamp timestamp-' + esc(context) + '" datetime="' + esc(iso) +
      '" title="' + esc(title) + '">' + esc(display) + '</time>'
    );
  }

  global.AuroraTimestamp = { render: render };
})(window);
