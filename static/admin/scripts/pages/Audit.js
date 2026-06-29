// Audit page (route: #mod/audit).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.8.

(function (global) {
  'use strict';

  let cursorStack = [];
  let nextCursor = null;
  let lastFilters = {};
  let subscription = null;

  // url-state wiring (§5.7.5) — the shared shape lives in AuroraListPage
  // (components/ListPage.js, #257). verifiedOnly is a boolean filter (applied
  // client-side post-fetch, below).
  const SCALAR_KEYS = ['actor', 'subject', 'subjectCid', 'action', 'source'];
  const BOOL_KEYS = ['verifiedOnly', 'ruleManagement', 'hookManagement', 'federationManagement'];

  function applyFilters(vals) {
    if (vals) {
      // Integration hooks (#350 / design-commit 26) + Federation Pattern-1
      // Phase E (#355 / design §5.3): the Integration-hook and Federation-
      // management filters are ONE-WAY-clear siblings — selecting either clears
      // the §5.5.4 source + rule-management filters; selecting a §5.5.4 filter
      // does NOT clear them (asymmetric, so the empty-intersection case arises).
      const turnedOnHook = vals.hookManagement && !(lastFilters && lastFilters.hookManagement);
      const turnedOnFed = vals.federationManagement && !(lastFilters && lastFilters.federationManagement);
      if (turnedOnHook || turnedOnFed) {
        vals.source = '';
        vals.ruleManagement = false;
      } else if (vals.source && vals.ruleManagement) {
        // §5.5.4 Phase E (MD-44): source vs rule-management stay mutually
        // exclusive; hookManagement is left untouched here.
        vals.ruleManagement = false;
      }
    }
    global.AuroraListPage.applyFilters(SCALAR_KEYS, BOOL_KEYS, vals, lastFilters && lastFilters.when, function (v) {
      lastFilters = v;
      cursorStack = [];
      nextCursor = null;
      refresh(null);
    });
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
    lastFilters = global.AuroraListPage.readFilters(SCALAR_KEYS, BOOL_KEYS, {});
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('audit-filter'),
        filters: [
          { type: 'text', id: 'actor', placeholder: 'Filter by actor DID' },
          { type: 'text', id: 'subject', placeholder: 'Filter by subject DID' },
          { type: 'text', id: 'subjectCid', placeholder: 'Filter by subject CID' },
          { type: 'text', id: 'action', placeholder: 'Filter by action' },
          // §5.5.4 Phase E (§6.4): substrate-action source filter.
          { type: 'select', id: 'source', label: 'Source', options: [
            { value: '', label: 'Any source' },
            { value: 'default_action', label: 'Default action' },
            { value: 'auto_label_rule', label: 'Auto-label rule' },
            { value: 'stale_expiration', label: 'Stale expiration' },
            { value: 'operator_removal', label: 'Operator removal' },
            { value: 'escalation', label: 'Escalation' },
            { value: 'system_diagnostic', label: 'System diagnostic' },
            { value: 'manual', label: 'Manual (operator)' },
            // Federation Pattern-1 Phase C/E (#353/#355): auto-accept-mode peer
            // additions during a discovery scan carry source=discovery.
            { value: 'discovery', label: 'Discovery (auto-accept)' },
          ] },
          // §5.5.4 Phase E (MD-40): Operator rule-management (rule-lifecycle).
          { type: 'checkbox', id: 'ruleManagement', label: 'Operator rule management' },
          // Integration hooks (#350): hook-lifecycle filter (one-way-clear sibling).
          { type: 'checkbox', id: 'hookManagement', label: 'Integration hooks' },
          // Federation Pattern-1 Phase E (#355): federation.* filter (sibling).
          { type: 'checkbox', id: 'federationManagement', label: 'Federation management' },
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
    subscription = global.AuroraListPage.subscribeModEvents(
      document.getElementById('audit-rt-indicator'),
      {
        onEvent: () => { if (cursorStack.length === 0) refresh(null); },
        onError: (e) => console.warn('audit subscription error:', e),
      },
    );
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
    // §5.5.4 Phase E — source / rule-management; Integration hooks (#350) —
    // hook-management. hook-management ANDs with a §5.5.4 filter only via the
    // asymmetric path (selecting a §5.5.4 filter while hook-management is on).
    if (lastFilters.hookManagement) params.hookManagement = true;
    if (lastFilters.federationManagement) params.federationManagement = true;
    if (lastFilters.ruleManagement) params.ruleManagement = true;
    else if (lastFilters.source) params.source = lastFilters.source;
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
        // Integration hooks (#350 / design-commit 34): explain the
        // AND-intersection-empty case when the hook filter is combined with a
        // §5.5.4 filter (reachable via the asymmetric one-way-clear).
        const mgmtAnd5554 = (lastFilters.hookManagement || lastFilters.federationManagement) &&
          (lastFilters.source || lastFilters.ruleManagement);
        const hookAndFed = lastFilters.hookManagement && lastFilters.federationManagement;
        const intersectionEmpty = mgmtAnd5554 || hookAndFed;
        const primary = intersectionEmpty
          ? 'No entries: a management filter (Integration hooks / Federation management) is combined (AND) with another filter, and their intersection is empty. Clear one filter to broaden.'
          : 'No audit entries match these filters.';
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: primary })
          : '<p class="empty-state">' + primary + '</p>';
        renderPagination();
        return;
      }
      const fmt = global.AuroraFormat;
      let html = '<table class="data-table"><thead><tr>' +
                 '<th>Seq</th><th>When</th><th>Actor</th><th>Action</th><th>Subject</th><th>Verified</th><th></th>' +
                 '</tr></thead><tbody>';
      // #359: no more window._auditCache — the detail page fetches each entry
      // server-side via getAuditEntry, so the list no longer seeds a global.
      for (const e of items) {
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
      global.AuroraErrorBoundary.mount(c, {
        message: 'Could not load audit: ' + ((e && e.message) || ''),
        onRetry: function () { refresh(cursor); },
      });
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
      // Annotate the v0.8→v0.9 format boundary, if present. chainLegacyCount
      // counts honestly-sealed entries that predate the #345 hash bump
      // (source/payload added to the canonical hash); they re-verify under
      // the prior form. This is NOT tamper — surface it as informational so
      // an operator reading a clean chain isn't left wondering why some rows
      // used an older format. Missing field (older server) → treated as 0.
      const legacyCount = typeof data.chainLegacyCount === 'number' ? data.chainLegacyCount : 0;
      if (legacyCount > 0) {
        const legacyLabel = legacyCount === 1 ? 'entry' : 'entries';
        detailHtml +=
          '<p class="chain-legacy-note">' + esc(String(legacyCount)) + ' ' + legacyLabel +
          ' verified under the legacy (pre-v0.9) hash format, sealed before the ' +
          'source/payload format change. These are untampered rows predating the ' +
          'format bump; linkage holds across the boundary.</p>';
      }
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
    global.AuroraListPage.renderPagination({
      container: document.getElementById('audit-pagination'),
      cursorStack: cursorStack,
      nextCursor: nextCursor,
      refresh: refresh,
    });
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modAudit', { mount: mount });
})(window);
