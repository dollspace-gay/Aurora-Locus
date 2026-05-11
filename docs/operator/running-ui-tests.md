# Running the admin UI test suite

The admin-UI substrate carries a small set of unit tests under
`static/admin/scripts/api/__tests__/`. They pin wire-format
contract decisions and capability-routing semantics that
broke in the past and would silently regress without coverage.

This document captures the invocation for operators / future
contributors. The friction it resolves: through Arc 6 Steps 2-4,
each step's completion report flagged "didn't run the tests
because the harness convention isn't documented." Step 5
sub-A recon resolved that — the tests run under bare Node with
no external framework, but the invocation isn't intuitive
because the project has no `package.json` and CI doesn't run
them.

## Prerequisites

- Node.js ≥ 18 (uses `node:test` and `node:assert/strict`,
  both core modules; Node ≥ 20 recommended for the stable
  test-runner exit-code semantics).
- No `npm install` required. The tests pull no external
  dependencies — they read source files directly via `fs.readFileSync`
  and stub `window`/`localStorage`/`fetch` minimally.

## Invocation

Run each test file directly:

```
node static/admin/scripts/api/__tests__/endpoints.test.js
node static/admin/scripts/api/__tests__/capabilities.test.js
```

The directory-discovery form (`node --test
static/admin/scripts/api/__tests__/`) does **not** work as
expected — Node treats the path as a single module to
`require` rather than a directory to walk. Run per-file.

A short script that runs both:

```sh
for t in static/admin/scripts/api/__tests__/*.test.js; do
  node "$t" || exit 1
done
```

Each file produces TAP output with per-test pass/fail and
a final summary block. Exit code 0 on all-pass, non-zero
on any failure.

## Expected pass count

As of Arc 6 Step 5 (cycle close trailing):

- `endpoints.test.js` — 4 tests (grantRole/revokeRole
  field-name contract pin across the role-management
  pages).
- `capabilities.test.js` — 8 tests (capability-routed
  substrate: cache TTL, endpoint resolution, refresh
  semantics, callEndpoint).

Total: 12 tests, all expected to pass on `main` and on
`skydeval/v0.4-cycle` tip.

## CI integration

These tests are **not** run by `.github/workflows/ci.yml`.
That workflow runs only the Rust-side cargo suites (lib +
postgres + multi-instance). The UI tests are run manually
during local development and during cycle-close audits.

A future cycle could add a node step to CI; the tests are
fast (~150ms total) and have no external dependencies,
so the gating cost is low. Not done in Arc 6 because the
UI substrate has been stable enough that ad-hoc local
runs sufficed.

## When to run

- Before every UI commit that touches
  `static/admin/scripts/api/` or any of the role-management
  / capability-consuming pages.
- During cycle-close audits as part of the broader
  "everything green" sweep.
- After upstream merges that may have touched the same
  surface.
