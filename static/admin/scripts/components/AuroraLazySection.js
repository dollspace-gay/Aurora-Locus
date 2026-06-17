// AuroraLazySection — defer a section's data fetch until it's needed
// (§10.5.1, the over-fetching-on-initial-load affordance). A page with
// below-the-fold sections registers each section's loader here instead of
// firing every fetch in one mount-time Promise.all; the loader runs when the
// section scrolls into viewport (IntersectionObserver), so an operator who
// clicks away quickly never pays for sections they didn't see.
//
//   const dispose = AuroraLazySection.observe(el, loadFn, { rootMargin, fallbackDelayMs })
//
//   - el        : the section's container element (must already be in the DOM
//                 — render the section's shell/skeleton eagerly, defer only its
//                 data fetch).
//   - loadFn    : zero-arg fn that fetches + renders the section. Called at
//                 most once.
//   - rootMargin: IntersectionObserver rootMargin (default '200px' — start the
//                 fetch slightly before the section is visible).
//   - fallbackDelayMs: when IntersectionObserver is unavailable, load after
//                 this delay instead of immediately (default 0 → immediate),
//                 the design's "simpler heuristic" path.
//
// Returns a disposer that disconnects the observer — a page MUST call it from
// unmount() so the observer itself doesn't outlive the page (a lazy-load
// primitive must not become the leak it prevents).

(function (global) {
  'use strict';

  function observe(el, loadFn, opts) {
    opts = opts || {};
    if (typeof loadFn !== 'function') return function () {};
    let fired = false;
    const fire = function () {
      if (fired) return;
      fired = true;
      try { loadFn(); } catch (e) { /* loader owns its own error surfacing */ }
    };

    // No element or no IntersectionObserver support → load per the fallback
    // (immediate, or after a short delay if requested).
    if (!el || typeof global.IntersectionObserver !== 'function') {
      const delay = opts.fallbackDelayMs || 0;
      if (delay > 0) {
        const tid = global.setTimeout(fire, delay);
        return function () { global.clearTimeout(tid); };
      }
      fire();
      return function () {};
    }

    const io = new global.IntersectionObserver(function (entries) {
      for (let i = 0; i < entries.length; i++) {
        if (entries[i].isIntersecting) {
          fire();
          io.disconnect();
          break;
        }
      }
    }, { rootMargin: opts.rootMargin || '200px' });
    io.observe(el);
    return function () { io.disconnect(); };
  }

  global.AuroraLazySection = { observe: observe };
})(window);
