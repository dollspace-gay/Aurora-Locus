// Shared list-page helpers (#257).
//
// Extracted from the moderation list pages — Appeals, Audit, Events, Reports —
// which had accumulated BYTE-IDENTICAL filter-state, cursor-pagination, and
// (Audit/Events) mod-events subscription wiring over the v0.9 cycle. The
// duplication was deliberate (build first, extract once the shape settled);
// the shape is now settled, so the verbatim routines live here once.
//
// Deliberately HELPERS, not a page factory. Behavior preservation is the
// load-bearing constraint, so each page keeps its own `mount`/`refresh`
// control flow and its module-level `cursorStack` / `nextCursor` /
// `lastFilters` state — only the three duplicated routines (`readFilters`,
// `applyFilters`, `renderPagination`) plus the shared subscribe call move
// here. Everything that varies per page — the endpoint, the params mapping,
// the column/card rendering, the empty/error copy, the bulk-bar — stays in the
// page. A page composes these helpers; it is not driven by them.
//
// Cursor paging only: every page that shares this pattern is cursor-based
// (`{ items, cursor }`); there is no page-number paging in the set. The
// load-all pages (Accounts/Invites/Queue) and the hybrid Sessions page do not
// use these helpers — they are a different shape and stay bespoke.

(function (global) {
  'use strict';

  // Seed a filters object from URL state (§5.7.5). Scalar keys copy through;
  // bool keys become `true`; `since`/`until` restore the `when` dateRange.
  // Identical to the per-page `readFilters` the four pages each carried.
  function readFilters(scalarKeys, boolKeys, defaults) {
    const u = global.AuroraUrlState ? global.AuroraUrlState.read() : {};
    const f = Object.assign({}, defaults || {});
    for (const k of scalarKeys) { if (u[k]) f[k] = u[k]; }
    for (const k of (boolKeys || [])) { if (u[k]) f[k] = true; }
    if (u.since || u.until) {
      f.when = { start: u.since ? new Date(u.since) : null, end: u.until ? new Date(u.until) : null };
    }
    return f;
  }

  // Write the filter values to URL state — which remounts the page, so
  // `readFilters` re-seeds from the query. `lastWhen` is the page's
  // `lastFilters.when`, the dateRange-fallback the pages applied when the
  // current values omit `when`. When `AuroraUrlState` is unavailable, the
  // `onLocal(vals)` fallback runs the page's own reset + refresh path
  // (preserving the pre-extraction local-apply behavior exactly).
  function applyFilters(scalarKeys, boolKeys, vals, lastWhen, onLocal) {
    const when = (vals && vals.when) || lastWhen || null;
    const u = {};
    for (const k of scalarKeys) { if (vals[k]) u[k] = vals[k]; }
    for (const k of (boolKeys || [])) { if (vals[k]) u[k] = '1'; }
    if (when && when.start) u.since = when.start.toISOString();
    if (when && when.end) u.until = when.end.toISOString();
    if (global.AuroraUrlState) global.AuroraUrlState.write(u);
    else if (onLocal) onLocal(vals);
  }

  // Render the cursor-stack prev/next strip via AuroraPagination. The page
  // owns `cursorStack` (the array is mutated in place here) and passes the
  // current `nextCursor` value plus its `refresh` fn. Behavior is identical to
  // the per-page `renderPagination`: `prev` pops (or empties the stack at
  // depth 1 and refreshes from the start), `next` pushes the next cursor.
  // `renderPagination` is re-invoked after every `refresh`, so the prev/next
  // closures always capture the current stack + cursor.
  function renderPagination(opts) {
    const c = opts.container;
    if (!c || !global.AuroraPagination) return;
    const cursorStack = opts.cursorStack;
    const nextCursor = opts.nextCursor;
    const refresh = opts.refresh;
    global.AuroraPagination.render({
      container: c,
      prevDisabled: cursorStack.length === 0,
      nextDisabled: !nextCursor,
      onPrev: function () {
        if (cursorStack.length > 1) {
          cursorStack.pop();
          const p = cursorStack[cursorStack.length - 1] || null;
          refresh(p);
        } else if (cursorStack.length === 1) {
          cursorStack.length = 0;
          refresh(null);
        }
      },
      onNext: function () {
        if (nextCursor) { cursorStack.push(nextCursor); refresh(nextCursor); }
      },
    });
  }

  // The live mod-events subscription Audit + Events share: subscribe to
  // `subscribe-mod-events` and attach the realtime indicator. Returns the
  // subscription handle, or `null` when `AuroraSubscription` is unavailable.
  // The page keeps its own already-subscribed dedup guard and its `onEvent`
  // handler (Audit refreshes the first page; Events prepends the live row).
  function subscribeModEvents(indicatorEl, handlers) {
    if (!global.AuroraSubscription) return null;
    const sub = global.AuroraSubscription.subscribe('subscribe-mod-events', {}, {
      onEvent: handlers.onEvent,
      onError: handlers.onError || function () {},
    });
    if (indicatorEl && sub) global.AuroraSubscription.attachIndicator(indicatorEl, sub);
    return sub;
  }

  global.AuroraListPage = {
    readFilters: readFilters,
    applyFilters: applyFilters,
    renderPagination: renderPagination,
    subscribeModEvents: subscribeModEvents,
  };
})(window);
