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
    // Phase B refinement: the embedded-id action subject-
    // variant check at aurora_admin.rs:910 emits this when the
    // request's named subject doesn't match the variant
    // expected by an action like ResolveAppeal{appealId} (the
    // appeal is bound to a specific subject; the request must
    // match). Distinct from InvalidEvent's wider per-arm
    // subject-shape check.
    SubjectVariantMismatch:
      "The action's required subject type doesn't match the " +
      "subject you selected. The action targets a specific " +
      "subject variant (account / record / blob); the request " +
      "supplied a different variant.",
    SubjectTargetMismatch:
      "The action targets a different subject than expected.",
    OrphanedAppeal:
      "This appeal has no current subject in the moderation " +
      "queue. The appeal may be stale, or the underlying " +
      "account or record has been deleted.",
    SubjectsArrayInvalidForAction:
      "This action only supports a single subject.",
    // `InvalidEvent` is deliberately NOT translated here (#300). The code is
    // OVERLOADED across unrelated subsystems: the moderation per-arm
    // subject-shape check (aurora_admin.rs:552) AND setRuntimeSetting's
    // unknown-key rejection ("unknown runtime setting key 'X'; known keys:
    // [...]", aurora_admin.rs:4079). A single canned translation keyed on the
    // code alone can only be right for one arm — the prior moderation-context
    // copy showed "TakedownRecord needs a record subject…" for a
    // runtime-setting save failure, which is actively misleading. The substrate
    // sends a specific, informative `message` in BOTH arms, so falling through
    // to it (translate's fallback) is more honest than any canned guess. If a
    // future arm needs friendlier prose, scope the translation per-endpoint or
    // give that arm a distinct error code rather than re-canning InvalidEvent.
    // Phase B addition: emitted by the AuroraJson extractor
    // (src/api/extractors.rs) when the request body fails JSON
    // deserialization. The original axum diagnostic is preserved
    // in the message field; this translation surfaces the
    // category to operators in actionable terms.
    InvalidRequestBody:
      "The request body has invalid structure. This usually " +
      "indicates a UI/server version mismatch or a malformed " +
      "field. Try reloading the page; if the error persists, " +
      "report the exact action you were taking.",
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
