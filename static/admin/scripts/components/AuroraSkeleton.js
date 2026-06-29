// AuroraSkeleton — layout-matching loading skeletons (§8.2.5, the standard
// loading state for content-bearing pages: lists, dashboards, detail views).
// Renders shimmer placeholders shaped like the eventual content so the
// operator sees structure during load rather than a blank flash or bare
// "Loading…" text. (Action-in-flight uses AuroraSpinner; route transitions
// use the route-progress bar.)
//
//   AuroraSkeleton.line(width)     → one shimmer text line (width: CSS length, default 100%)
//   AuroraSkeleton.lines(n)        → n shimmer lines (last one short, as prose wraps)
//   AuroraSkeleton.card({ lines }) → a settings-card-shaped skeleton (title bar + lines)
//   AuroraSkeleton.cards(n, opts)  → n card skeletons (for a grid placeholder)

(function (global) {
  'use strict';

  function line(width) {
    const w = width ? ' style="width:' + String(width).replace(/"/g, '') + '"' : '';
    return '<div class="skeleton skeleton-line"' + w + '></div>';
  }

  function lines(n) {
    n = n || 3;
    let out = '';
    for (let i = 0; i < n; i++) {
      // Last line shorter, mimicking wrapped prose.
      out += line(i === n - 1 ? '60%' : null);
    }
    return out;
  }

  function card(opts) {
    opts = opts || {};
    const n = opts.lines || 3;
    return (
      '<div class="settings-card skeleton-card" aria-hidden="true">' +
      '<div class="skeleton skeleton-title"></div>' +
      lines(n) +
      '</div>'
    );
  }

  function cards(n, opts) {
    n = n || 3;
    let out = '';
    for (let i = 0; i < n; i++) out += card(opts);
    return out;
  }

  global.AuroraSkeleton = { line: line, lines: lines, card: card, cards: cards };
})(window);
