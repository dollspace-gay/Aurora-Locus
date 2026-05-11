// Server error-code → operator-friendly message translation.
//
// Per docs/V04_DESIGN.md §5.3.1 (Arc 6 Step 1). v0.3 added
// structured 4xx error codes (`SubjectVariantMismatch`,
// `SubjectTargetMismatch`, `OrphanedAppeal`,
// `SubjectsArrayInvalidForAction`) on the wire; pre-Arc-6
// `client.js` rendered them as `HTTP 400: <raw>` regardless,
// dropping the operator-facing nuance. This module is the
// central table consumed by `client.js`'s 4xx path: known
// codes map to a friendlier prose template; unknown codes
// fall back to the server's `message` field unchanged.
//
// New codes shipping on the backend land here as a one-line
// table entry. The structure is i18n-future-proofed (a future
// locale-aware renderer can replace `TABLE` with a per-locale
// lookup) but Step 1 ships only the prose-only English path.

(function (global) {
  'use strict';

  const TABLE = {
    SubjectVariantMismatch:
      "The action's required subject type doesn't match the " +
      "subject you selected.",
    SubjectTargetMismatch:
      "The action targets a different subject than expected.",
    OrphanedAppeal:
      "This appeal has no current subject in the moderation " +
      "queue. The appeal may be stale, or the underlying " +
      "account or record has been deleted.",
    SubjectsArrayInvalidForAction:
      "This action only supports a single subject.",
  };

  // translate(code, fallback) — looks up `code` in TABLE.
  // Returns the operator-friendly message if found; otherwise
  // returns `fallback` (typically the raw server `message`),
  // or `code` itself if no fallback was supplied. Always
  // returns a string.
  function translate(code, fallback) {
    if (typeof code === 'string' && TABLE[code]) {
      return TABLE[code];
    }
    return fallback != null ? fallback : (code || '');
  }

  // has(code) — predicate. True if a translation exists.
  // Callers use this to decide whether to surface a "details"
  // affordance that reveals the raw server message alongside
  // the translated one (a translated message hides nuance the
  // operator may still want; the raw fallback message doesn't).
  function has(code) {
    return typeof code === 'string' && TABLE[code] != null;
  }

  global.AuroraErrorTranslations = {
    translate: translate,
    has: has,
    table: TABLE,
  };
})(window);
