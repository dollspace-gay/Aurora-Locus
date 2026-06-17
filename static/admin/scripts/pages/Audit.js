// Audit page (route: #mod/audit).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.8.

(function (global) {
  'use strict';

  let cursorStack = [];
  let nextCursor = null;
  let lastFilters = {};
  let subscription = null;

  // url-state wiring (§5.7.5) — see Reports.js for the shared shape.
  // verifiedOnly is a boolean filter (applied client-side post-fetch).
  const SCALAR_KEYS = ['actor', 'subject', 'subjectCid', 'action'];
  const BOOL_KEYS = ['verifiedOnly'];

  function readFilters(defaults) {
    const u = global.AuroraUrlState ? global.AuroraUrlState.read() : {};
    const f = Object.assign({}, defaults || {});
    for (const k of SCALAR_KEYS) { if (u[k]) f[k] = u[k]; }
    for (const k of BOOL_KEYS) { if (u[k]) f[k] = true; }
    if (u.since || u.until) {
      f.when = { start: u.since ? new Date(u.since) : null, end: u.until ? new Date(u.until) : null };
    }
    return f;
  }

  function applyFilters(vals) {
    const when = (vals && vals.when) || (lastFilters && lastFilters.when) || null;
    const u = {};
    for (const k of SCALAR_KEYS) { if (vals[k]) u[k] = vals[k]; }
    for (const k of BOOL_KEYS) { if (vals[k]) u[k] = '1'; }
    if (when && when.start) u.since = when.start.toISOString();
    if (when && when.end) u.until = when.end.toISOString();
    if (global.AuroraUrlState) global.AuroraUrlState.write(u);
    else { lastFilters = vals; cursorStack = []; nextCursor = null; refresh(null); }
  }

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Audit Trail</h2><p class="page-subtitle">Hash-chained audit log via tools.aurora.admin.getAuditTrail</p></div>' +
      '  <div class="audit-header-indicators">' +
      '    <div id="audit-chain-indicator" class="chain-indicator"></div>' +
      '    <div id="audit-rt-indicator" class="rt-indicator-slot"></div>' +
      '  </div>' +
      '</header>' +
      '<div id="audit-chain-detail" class="chain-indicator-detail" hidden></div>' +
      '<div id="audit-filter"></div>' +
      '<p class="filter-url-hint">' + (global.t ? global.t('common.filters_in_url') : '') + '</p>' +
      '<div id="audit-table-container"></div>' +
      '<div id="audit-pagination"></div>';
    cursorStack = [];
    nextCursor = null;
    lastFilters = readFilters({});
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('audit-filter'),
        filters: [
          { type: 'text', id: 'actor', placeholder: 'Filter by actor DID' },
          { type: 'text', id: 'subject', placeholder: 'Filter by subject DID' },
          { type: 'text', id: 'subjectCid', placeholder: 'Filter by subject CID' },
          { type: 'text', id: 'action', placeholder: 'Filter by action' },
          { type: 'checkbox', id: 'verifiedOnly', label: 'Verified only' },
          { type: 'dateRange', id: 'when', label: 'Date range' },
        ],
        initial: lastFilters,
        onApply: applyFilters,
      });
    }
    await refresh(null);
    startSubscription();
    return {
      unmount: () => {
        if (subscription) { try { subscription.unsubscribe(); } catch (e) {} subscription = null; }
      },
    };
  }

  function startSubscription() {
    if (subscription || !global.AuroraSubscription) return;
    const indicator = document.getElementById('audit-rt-indicator');
    subscription = global.AuroraSubscription.subscribe('subscribe-mod-events', {}, {
      onEvent: () => { if (cursorStack.length === 0) refresh(null); },
      onError: (e) => console.warn('audit subscription error:', e),
    });
    if (indicator) global.AuroraSubscription.attachIndicator(indicator, subscription);
  }

  async function refresh(cursor) {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('audit-table-container');
    if (!c || !ep) return;
    const params = { limit: 25 };
    if (lastFilters.actor) params.actorDid = lastFilters.actor;
    if (lastFilters.subject) params.subjectDid = lastFilters.subject;
    if (lastFilters.subjectCid) params.subjectCid = lastFilters.subjectCid;
    if (lastFilters.action) params.action = lastFilters.action;
    if (cursor) params.cursor = cursor;
    if (lastFilters.when && lastFilters.when.start) params.since = lastFilters.when.start.toISOString();
    if (lastFilters.when && lastFilters.when.end) params.until = lastFilters.when.end.toISOString();
    c.innerHTML = global.AuroraSkeleton.cards(3);
    try {
      const data = await ep.admin.getAuditTrail(params);
      let items = (data && data.items) || [];
      nextCursor = data && data.cursor;
      renderChainIndicator(data);
      if (lastFilters.verifiedOnly) items = items.filter((e) => e.verified);
      if (items.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No audit entries match these filters.' })
          : '<p class="empty-state">No entries.</p>';
        renderPagination();
        return;
      }
      const fmt = global.AuroraFormat;
      let html = '<table class="data-table"><thead><tr>' +
                 '<th>Seq</th><th>When</th><th>Actor</th><th>Action</th><th>Subject</th><th>Verified</th><th></th>' +
                 '</tr></thead><tbody>';
      window._auditCache = window._auditCache || {};
      for (const e of items) {
        window._auditCache[e.id] = e;
        const subj = e.subjectRef ? (e.subjectRef.did || e.subjectRef.uri || e.subjectRef.cid || '—') : '—';
        const verifiedBadge = e.verified
          ? '<span class="status-badge status-verified" title="Hash matches stored chain hash">✓ verified</span>'
          : '<span class="status-badge status-suspended" title="Hash does not match — possibly tampered or pre-chain">✗ unverified</span>';
        html += '<tr>' +
                '<td>' + esc(e.sequence) + '</td>' +
                '<td>' + global.AuroraTimestamp.render({ value: e.timestamp, context: 'forensic' }) + '</td>' +
                '<td>' + (e.actorDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(e.actorDid) : '<code>' + esc(e.actorDid) + '</code>') : '—') + '</td>' +
                '<td>' + esc(e.action) + '</td>' +
                '<td><code>' + esc(subj) + '</code></td>' +
                '<td>' + verifiedBadge + '</td>' +
                '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.audit(e.id) : '#' + esc(e.id)) + '</td>' +
                '</tr>';
      }
      html += '</tbody></table>';
      c.innerHTML = html;
      renderPagination();
    } catch (e) {
      c.innerHTML = '<p class="empty-state">Could not load audit: ' + esc(e && e.message) + '</p>';
    }
  }

  // Render the chain-verification status indicator at the top of the
  // audit page, plus a click-to-expand detail panel. Per V04_DESIGN.md
  // §5.3.5 case (a): getAuditTrail surfaces both top-level chainVerified
  // (the whole-chain linkage check) and chainVerifiedThrough (the head
  // sequence on success, or failing_sequence - 1 on failure). The
  // indicator reads top-level fields directly; per-row `verified` is
  // already surfaced inline on each row.
  //
  // Three states:
  //   chainVerified === true                              → green ✓
  //   chainVerified === false && chainVerifiedThrough > 0 → yellow ⚠
  //   chainVerified === false && chainVerifiedThrough===0 → red ✗
  //
  // Missing fields (pre-v0.3 server, or response shape change) → omit
  // the indicator silently rather than render a "no data" placeholder.
  // Matches the optional-global guard pattern Step 1 established.
  //
  // Loading state: between filter change and next response, the
  // indicator retains the previous state. This is least jarring — the
  // chain doesn't change between refreshes, so the previous verdict is
  // still informative until the next response replaces it.
  function renderChainIndicator(data) {
    const slot = document.getElementById('audit-chain-indicator');
    const detail = document.getElementById('audit-chain-detail');
    if (!slot || !detail) return;
    if (!data || typeof data.chainVerified !== 'boolean'
        || typeof data.chainVerifiedThrough !== 'number') {
      slot.innerHTML = '';
      detail.hidden = true;
      return;
    }
    const verified = data.chainVerified;
    const through = data.chainVerifiedThrough;

    let badgeClass;
    let icon;
    let label;
    let tooltip;
    let detailHtml;
    if (verified) {
      badgeClass = 'chain-indicator-ok';
      icon = '✓';
      label = 'Chain verified through entry ' + through;
      tooltip = 'The audit chain has been verified end-to-end through sequence ' + through + '.';
      detailHtml =
        '<p><strong>Chain verified.</strong> The hash chain from sequence 1 ' +
        'through sequence ' + esc(through) + ' has been re-verified end-to-end ' +
        'on this request. Per-row hash and per-row linkage both match.</p>';
    } else if (through > 0) {
      badgeClass = 'chain-indicator-warn';
      icon = '⚠';
      label = 'Chain verified through entry ' + through + '; failure at entry ' + (through + 1);
      tooltip = 'Chain verification failed at sequence ' + (through + 1) +
                '. Entries through ' + through + ' are verified.';
      detailHtml =
        '<p><strong>Chain verification failed.</strong> The chain is verified ' +
        'through sequence <code>' + esc(through) + '</code>; verification broke at ' +
        'sequence <code>' + esc(through + 1) + '</code>. Entries above the break ' +
        'cannot be relied on without investigation.</p>' +
        chainBrokenCommandBlock();
    } else {
      badgeClass = 'chain-indicator-bad';
      icon = '✗';
      label = 'Chain verification failed at the first entry';
      tooltip = 'Chain verification failed at sequence 1. No entries are verified.';
      detailHtml =
        '<p><strong>Chain verification failed at sequence 1.</strong> The first ' +
        'audit-chain entry failed verification, so no entries above are reliable. ' +
        'This is unusual; the entry may have been tampered with, or the chain ' +
        'genesis may have been corrupted.</p>' +
        chainBrokenCommandBlock();
    }

    slot.innerHTML =
      '<button type="button" class="chain-indicator-badge ' + badgeClass + '" ' +
      'aria-expanded="false" aria-controls="audit-chain-detail" ' +
      'title="' + esc(tooltip) + '">' +
      '<span class="chain-indicator-icon" aria-hidden="true">' + icon + '</span>' +
      '<span class="chain-indicator-label">' + esc(label) + '</span>' +
      '</button>';

    detail.innerHTML = detailHtml;
    // The detail panel itself stays hidden until the badge is clicked.
    detail.hidden = true;

    const btn = slot.querySelector('.chain-indicator-badge');
    if (btn) {
      btn.addEventListener('click', () => {
        const open = !detail.hidden;
        detail.hidden = open;
        btn.setAttribute('aria-expanded', open ? 'false' : 'true');
      });
    }
  }

  // CLI-suggestion code block for broken-chain states. Per Q10 recon,
  // `aurora-locus debug verify-audit-chain` is the operator's
  // diagnostic; the UI surfaces the command name but does not execute.
  function chainBrokenCommandBlock() {
    return '<p>Run the chain-walk diagnostic against this PDS to ' +
           'investigate the break:</p>' +
           '<pre class="chain-indicator-cmd"><code>aurora-locus debug verify-audit-chain</code></pre>';
  }

  function renderPagination() {
    const c = document.getElementById('audit-pagination');
    if (!c || !global.AuroraPagination) return;
    global.AuroraPagination.render({
      container: c,
      prevDisabled: cursorStack.length === 0,
      nextDisabled: !nextCursor,
      onPrev: () => {
        if (cursorStack.length > 1) {
          cursorStack.pop();
          const p = cursorStack[cursorStack.length - 1] || null;
          refresh(p);
        } else if (cursorStack.length === 1) { cursorStack = []; refresh(null); }
      },
      onNext: () => { if (nextCursor) { cursorStack.push(nextCursor); refresh(nextCursor); } },
    });
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modAudit', { mount: mount });
})(window);
