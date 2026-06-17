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

    const cached = (window._auditCache || {})[id];
    if (cached) renderBody(cached);
    else {
      try {
        // Server-side getAuditEntry would ideally exist; fall back to
        // listing first 25 with the id filter and matching client-side.
        const data = await global.AuroraEndpoints.admin.getAuditTrail({ limit: 100 });
        const items = (data && data.items) || [];
        const entry = items.find((e) => String(e.id) === String(id));
        if (entry) renderBody(entry);
        else document.getElementById('aed-body').innerHTML =
          '<p class="empty-state">Entry not in current page. Use Audit page filters to narrow.</p>';
      } catch (e) {
        document.getElementById('aed-body').innerHTML =
          '<p class="empty-state">Could not load entry: ' + esc(e && e.message) + '</p>';
      }
    }
    return {};
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

  function walkChainTo(prevHash) {
    const items = window._auditCache || {};
    const target = Object.values(items).find((e) => e.currentHash === prevHash);
    if (target) {
      if (global.AuroraRouter) global.AuroraRouter.navigate('mod/audit/' + encodeURIComponent(target.id));
      return;
    }
    if (global.AuroraToast) global.AuroraToast.warning('Previous entry not in current page cache. Use filters to narrow to the previous range.');
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('modAuditDetail', { mount: mount });
})(window);
