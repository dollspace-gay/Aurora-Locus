// EntityRef substrate primitive (substrate primitive 1).
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §6.1: a single component for
// rendering an account/record/blob/event reference. Resolves DIDs to
// handles when available, displays both, and provides a click-through
// to the canonical detail page.
//
// API:
//   AuroraEntityRef.account(did, handle?)  → '<a class="entity-ref" href="#ops/accounts/...">…</a>'
//   AuroraEntityRef.record(uri)            → '<a class="entity-ref" href="#ops/records/…">…</a>'
//   AuroraEntityRef.blob(cid)              → '<a class="entity-ref" href="#ops/blobs/…">…</a>'
//   AuroraEntityRef.event(id)              → '<a class="entity-ref" href="#mod/events/…">…</a>'
//   AuroraEntityRef.appeal(id)             → '<a class="entity-ref" href="#mod/appeals/…">…</a>'
//   AuroraEntityRef.audit(id)              → '<a class="entity-ref" href="#mod/audit/…">…</a>'

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  function account(did, handle) {
    if (!did) return '<span class="entity-ref">—</span>';
    // CLI sentinel: actor strings prefixed with 'cli:' come from
    // command-line invocations (bootstrap, gc-sweep, etc.) and
    // have no runtime-PDS account to navigate to. Render as a
    // non-clickable badge per V04_DESIGN §5.3.4. Defensive
    // typeof check — non-string DIDs fall through to the
    // existing rendering path's null-handling.
    if (typeof did === 'string' && did.startsWith('cli:')) {
      return _renderCliSentinel(did);
    }
    const display = handle ? '@' + handle : '';
    return '<a class="entity-ref" href="#ops/accounts/' + encodeURIComponent(did) + '">' +
           (display ? '<span>' + esc(display) + '</span> ' : '') +
           '<code>' + esc(shortDid(did)) + '</code>' +
           '</a>';
  }

  // Render a CLI-actor sentinel as a non-clickable badge. The 'cli:'
  // prefix is stripped from the displayed label; the suffix is the
  // CLI command / identity name (e.g. 'bootstrap', 'gc-sweep'). Uses
  // both `entity-ref` (shared inline-flex positioning) and
  // `entity-ref--cli` (sentinel-specific styling) classes so the
  // badge participates in the surrounding entity-ref layout while
  // visually distinguishing itself from clickable runtime-account
  // links.
  function _renderCliSentinel(did) {
    const suffix = did.slice(4);
    return '<span class="entity-ref entity-ref--cli">CLI: ' +
           esc(suffix) + '</span>';
  }

  function record(uri) {
    if (!uri) return '<span class="entity-ref">—</span>';
    return '<a class="entity-ref" href="#ops/records/' + encodeURIComponent(uri) + '">' +
           '<code>' + esc(uri) + '</code>' +
           '</a>';
  }

  function blob(cid) {
    if (!cid) return '<span class="entity-ref">—</span>';
    return '<a class="entity-ref" href="#ops/blobs/' + encodeURIComponent(cid) + '">' +
           '<code>' + esc(shortCid(cid)) + '</code></a>';
  }

  function event(id) {
    if (id == null) return '<span class="entity-ref">—</span>';
    return '<a class="entity-ref" href="#mod/events/' + encodeURIComponent(id) + '">#' + esc(id) + '</a>';
  }

  function appeal(id) {
    if (id == null) return '<span class="entity-ref">—</span>';
    return '<a class="entity-ref" href="#mod/appeals/' + encodeURIComponent(id) + '">#' + esc(id) + '</a>';
  }

  function audit(id) {
    if (id == null) return '<span class="entity-ref">—</span>';
    return '<a class="entity-ref" href="#mod/audit/' + encodeURIComponent(id) + '">#' + esc(id) + '</a>';
  }

  function invite(code) {
    if (!code) return '<span class="entity-ref">—</span>';
    return '<a class="entity-ref" href="#ops/invites/' + encodeURIComponent(code) + '">' +
           '<code>' + esc(code) + '</code></a>';
  }

  // Render whichever entity type matches the structured subject. The
  // ATProto subject discriminator decides; falls back to text when no
  // recognized shape is found.
  function fromSubject(subject) {
    if (!subject || typeof subject !== 'object') {
      return subject ? esc(String(subject)) : '—';
    }
    const t = subject.$type || subject['$type'];
    if (typeof t === 'string') {
      if (t.endsWith('repoRef')) return account(subject.did);
      if (t.endsWith('strongRef')) return record(subject.uri);
      if (t.endsWith('repoBlobRef')) return blob(subject.cid);
    }
    if (subject.did) return account(subject.did);
    if (subject.uri) return record(subject.uri);
    if (subject.cid) return blob(subject.cid);
    return esc(JSON.stringify(subject));
  }

  function shortDid(did) {
    if (!did) return '';
    if (did.length <= 32) return did;
    return did.slice(0, 16) + '…' + did.slice(-8);
  }

  function shortCid(cid) {
    if (!cid) return '';
    if (cid.length <= 24) return cid;
    return cid.slice(0, 12) + '…' + cid.slice(-6);
  }

  global.AuroraEntityRef = {
    account: account,
    record: record,
    blob: blob,
    event: event,
    appeal: appeal,
    audit: audit,
    invite: invite,
    fromSubject: fromSubject,
    shortDid: shortDid,
    shortCid: shortCid,
  };
})(window);
