// Audit entry detail page (route: #mod/audit/:id).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.9.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const id = params && params.id;
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#mod/audit">Moderation</a> <span class="breadcrumb-sep">›</span> <a href="#mod/audit">Audit</a> <span class="breadcrumb-sep">›</span> #' + esc(id) + '</nav>' +
      '<header class="page-header"><div><h2>Audit entry #' + esc(id) + '</h2></div></header>' +
      '<div id="aed-body">' + global.AuroraSkeleton.lines(4) + '</div>';

    // #359: always fetch from the server (getAuditEntry). The page no longer
    // depends on a list-populated window._auditCache, so a deep link or a
    // refresh resolves the entry directly rather than degrading on a miss.
    await loadEntry(id);
    return {};
  }

  async function loadEntry(id) {
    const body = document.getElementById('aed-body');
    if (!body) return;
    body.innerHTML = global.AuroraSkeleton.lines(4);
    try {
      const entry = await global.AuroraEndpoints.admin.getAuditEntry({ id: id });
      renderBody(entry);
    } catch (e) {
      // A 404 (entry id doesn't exist) is a distinct, expected outcome from a
      // transport failure — surface it as an empty state, not a retry boundary.
      if (e && e.status === 404) {
        body.innerHTML = '<p class="empty-state">No audit entry #' + esc(id) + '.</p>';
        return;
      }
      global.AuroraErrorBoundary.mount(body, {
        message: 'Could not load entry: ' + ((e && e.message) || ''),
        onRetry: function () { loadEntry(id); },
      });
    }
  }

  function renderBody(e) {
    const fmt = global.AuroraFormat;
    const subjStr = e.subjectRef ? JSON.stringify(e.subjectRef, null, 2) : 'none';
    const prevHash = e.previousHash;
    const prevHashSection = prevHash
      ? '<p><strong>Previous hash:</strong> <code>' + esc(prevHash) + '</code> ' +
        '<a href="javascript:void(0)" id="aed-walk">[walk to previous]</a></p>'
      : '<p><strong>Previous hash:</strong> none (first entry in chain)</p>';
    const cascadeSection = renderCascadeSection(e);
    const body = document.getElementById('aed-body');
    body.innerHTML =
      '<div class="settings-card">' +
      '  <h3>Entry detail</h3>' +
      '  <dl style="font-size: 0.875rem;">' +
      '    <dt>Sequence</dt><dd>' + esc(e.sequence) + '</dd>' +
      '    <dt>Timestamp</dt><dd>' + global.AuroraTimestamp.render({ value: e.timestamp, context: 'forensic' }) + '</dd>' +
      '    <dt>Actor DID</dt><dd>' + (e.actorDid ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(e.actorDid) : '<code>' + esc(e.actorDid) + '</code>') : '—') + '</dd>' +
      '    <dt>Action</dt><dd>' + esc(e.action) + '</dd>' +
      '    <dt>Rationale</dt><dd>' + esc(e.rationale) + '</dd>' +
      '    <dt>Subject</dt><dd><pre style="white-space: pre-wrap; margin: 0;">' + esc(subjStr) + '</pre></dd>' +
      '    <dt>Snapshot ID</dt><dd>' + esc(e.snapshotId || 'none') + '</dd>' +
      '    <dt>Event ID</dt><dd>' + (e.eventId ? (global.AuroraEntityRef ? global.AuroraEntityRef.event(e.eventId) : '#' + esc(e.eventId)) : 'none') + '</dd>' +
      '    <dt>Current hash</dt><dd><code style="word-break: break-all;">' + esc(e.currentHash) + '</code></dd>' +
      '  </dl>' +
      cascadeSection +
      '  ' + prevHashSection +
      '  <p><strong>Verified:</strong> ' + (e.verified ? '✓ Yes — recomputed hash matches stored value' : '✗ No — hash divergent or pre-chain sentinel') + '</p>' +
      '</div>';

    const walkBtn = document.getElementById('aed-walk');
    if (walkBtn) walkBtn.addEventListener('click', () => walkChainTo(e.previousHash));
  }

  // Render the cascade-subjects section. Returns the empty string when
  // the entry has no cascade (single-subject actions have no cascade,
  // batch actions populate cascadeSubjects + cascadeSnapshotIds paired
  // by index). Per V04_DESIGN.md §5.4.3 sub-3a, no "no cascades"
  // placeholder — the section is omitted entirely.
  //
  // cascadeSnapshotIds[i] is Option<String> on the wire (JS: string or
  // null); null means the subject at that index wasn't snapshottable.
  // Subject click-throughs route to the per-variant detail page via
  // AuroraEntityRef.fromSubject. Snapshot IDs render as plain code
  // (no detail-page route exists for snapshot IDs today).
  function renderCascadeSection(e) {
    const subjects = Array.isArray(e.cascadeSubjects) ? e.cascadeSubjects : [];
    const snapshotIds = Array.isArray(e.cascadeSnapshotIds) ? e.cascadeSnapshotIds : [];
    if (subjects.length === 0) return '';
    const items = subjects.map((subj, i) => {
      const subjHtml = global.AuroraEntityRef
        ? global.AuroraEntityRef.fromSubject(subj)
        : '<code>' + esc(JSON.stringify(subj)) + '</code>';
      const sid = snapshotIds[i];
      const snapHtml = sid != null
        ? ' — snapshot <code>' + esc(sid) + '</code>'
        : ' — <em>no snapshot</em>';
      return '<li>' + subjHtml + snapHtml + '</li>';
    }).join('');
    return '<p><strong>Cascade subjects (' + subjects.length + '):</strong></p>' +
           '<ul class="audit-cascade-list">' + items + '</ul>';
  }

  // #359: resolve the previous entry server-side by its hash (getAuditEntry's
  // `hash` selector) and navigate to it — no longer bounded to entries already
  // in a page cache, so the walk reaches arbitrarily far back.
  async function walkChainTo(prevHash) {
    if (!prevHash) return;
    try {
      const target = await global.AuroraEndpoints.admin.getAuditEntry({ hash: prevHash });
      if (target && target.id != null && global.AuroraRouter) {
        global.AuroraRouter.navigate('mod/audit/' + encodeURIComponent(target.id));
      }
    } catch (e) {
      if (global.AuroraToast) {
        global.AuroraToast.warning(
          e && e.status === 404
            ? 'Previous entry not found (it may have been pruned).'
            : 'Could not load the previous entry: ' + ((e && e.message) || ''),
        );
      }
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modAuditDetail', { mount: mount });
})(window);
