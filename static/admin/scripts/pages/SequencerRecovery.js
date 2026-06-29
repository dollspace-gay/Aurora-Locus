// Sequencer recovery page (Arc H §7.4.2 / #295). Route #ops/sequencer/recovery,
// SuperAdmin only, visible in full + reduced modes (via the Operations domain
// gate). The escalation surface for sequencer-level intervention beyond the
// routine Sequencer page (§5.4.4) pause/resume/reset controls.
//
// v0.9 ships ONE operation: a read-only DEEP INTEGRITY VALIDATION. It surfaces
// undecodable event blobs (the firehose silently drops them) and per-DID rev
// non-monotonicity (concurrent-write ordering bugs) the substrate otherwise
// hides. Being read-only, the run has no destructive-confirm — it's a
// diagnostic. Layout per §7.4.2: sequencer-state panel + the per-operation card.
// Consumes the #294 XRPCs (sequencerRecoveryOptions / runSequencerRecovery /
// getSequencerRecoveryProgress / cancelSequencerRecovery).

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);
  const EP = () => global.AuroraEndpoints.superadmin;
  function ts(value, context) {
    return global.AuroraTimestamp.render({ value: value, context: context });
  }

  let poll = null;

  function clearPoll() {
    if (poll) clearTimeout(poll);
    poll = null;
  }

  function el(id) { return document.getElementById(id); }

  function kv(label, valueHtml) {
    return '<div class="kv-row"><dt>' + esc(label) + '</dt><dd>' + valueHtml + '</dd></div>';
  }

  async function mount({ container }) {
    clearPoll();
    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#ops/sequencer">' + esc(t('seqRecovery.crumb')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('seqRecovery.title')) +
      '</nav>' +
      '<header class="page-header"><div><h2>' + esc(t('seqRecovery.title')) +
      ' <span class="role-tag">SuperAdmin only</span></h2>' +
      '<p class="page-subtitle">' + esc(t('seqRecovery.subtitle')) + '</p></div></header>' +
      '<div class="settings-card" id="sr-state">' + global.AuroraSkeleton.lines(2) + '</div>' +
      '<div class="settings-card" id="sr-op">' + global.AuroraSkeleton.lines(2) + '</div>';

    await renderState();
    await renderOp();
    return { unmount: clearPoll };
  }

  async function renderState() {
    const card = el('sr-state');
    if (!card) return;
    let o;
    try { o = await EP().sequencerRecoveryOptions(); }
    catch (e) {
      card.innerHTML = '<h3>' + esc(t('seqRecovery.state_heading')) + '</h3><div id="sr-state-err"></div>';
      global.AuroraInlineError.mount(el('sr-state-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: renderState,
      });
      return;
    }
    const s = o.state || {};
    let html =
      '<h3>' + esc(t('seqRecovery.state_heading')) + '</h3>' +
      '<dl class="kv-list">' +
      kv(t('seqRecovery.total_rows'), esc(String(s.totalRows != null ? s.totalRows : 0))) +
      kv(t('seqRecovery.invalidated_rows'), esc(String(s.invalidatedRows != null ? s.invalidatedRows : 0))) +
      kv(t('seqRecovery.head_seq'), esc(s.headSeq != null ? String(s.headSeq) : '—')) +
      kv(t('seqRecovery.min_seq'), esc(s.minSeq != null ? String(s.minSeq) : '—')) +
      '</dl>';
    if (o.lastValidation) {
      const lv = o.lastValidation;
      html += '<p class="settings-help">' + esc(t('seqRecovery.last_validation', {
        outcome: lv.outcome || '—',
        malformed: lv.malformedCount || 0,
        nonMonotonic: lv.nonMonotonicCount || 0,
      })) + '</p>';
    }
    card.innerHTML = html;
  }

  async function renderOp() {
    const card = el('sr-op');
    if (!card) return;
    let p;
    try { p = await EP().getSequencerRecoveryProgress(); }
    catch (e) {
      card.innerHTML = '<h3>' + esc(t('seqRecovery.op_heading')) + '</h3><div id="sr-op-err"></div>';
      global.AuroraInlineError.mount(el('sr-op-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: renderOp,
      });
      return;
    }
    if (p && p.running) {
      renderRunning(p);
    } else {
      renderIdle(p);
    }
  }

  function renderIdle(p) {
    const card = el('sr-op');
    if (!card) return;
    card.innerHTML =
      '<h3>' + esc(t('seqRecovery.op_heading')) + '</h3>' +
      '<p class="settings-help">' + esc(t('seqRecovery.op_explainer')) + '</p>' +
      '<div class="form-actions"><button class="btn btn-primary" id="sr-run">' +
        esc(t('seqRecovery.run_button')) + '</button></div>' +
      (p && p.report ? reportHtml(p) : '');
    const b = el('sr-run');
    if (b) b.addEventListener('click', doRun);
    const rb = el('sr-route');
    if (rb) {
      const affected = (p && p.report && p.report.affectedDids) || [];
      rb.addEventListener('click', () => doRouteMalformed(affected.length));
    }
  }

  function reportHtml(p) {
    const r = p.report || {};
    const malformed = r.malformed || [];
    const nonMono = r.nonMonotonic || [];
    let html =
      '<h4>' + esc(t('seqRecovery.report_heading')) + '</h4>' +
      '<p class="settings-help">' + esc(t('seqRecovery.report_summary', {
        outcome: p.lastOutcome || '—',
        scanned: r.rowsScanned || 0,
        malformed: r.malformedCount || 0,
        nonMonotonic: r.nonMonotonicCount || 0,
      })) + '</p>';
    if ((r.malformedCount || 0) === 0 && (r.nonMonotonicCount || 0) === 0) {
      html += '<p class="settings-help">' + esc(t('seqRecovery.report_clean')) + '</p>';
      return html;
    }
    if (malformed.length) {
      html += '<h5>' + esc(t('seqRecovery.malformed_heading')) + '</h5>' +
        '<table class="data-table"><thead><tr><th>' + esc(t('seqRecovery.col_seq')) +
        '</th><th>' + esc(t('seqRecovery.col_did')) + '</th><th>' +
        esc(t('seqRecovery.col_event_type')) + '</th></tr></thead><tbody>' +
        malformed.map((m) => '<tr><td>' + esc(String(m.seq)) + '</td><td><code>' +
          esc(m.did) + '</code></td><td>' + esc(m.eventType) + '</td></tr>').join('') +
        '</tbody></table>';
    }
    const affected = r.affectedDids || [];
    if (affected.length) {
      html += '<h5>' + esc(t('seqRecovery.affected_heading')) + '</h5>' +
        '<p class="settings-help">' + esc(t('seqRecovery.affected_help', { count: affected.length })) + '</p>' +
        '<ul class="did-list">' +
        affected.map((d) => '<li><a href="#ops/repo-rebuild/' + encodeURIComponent(d) +
          '"><code>' + esc(d) + '</code></a></li>').join('') +
        '</ul>' +
        '<div class="form-actions"><button class="btn btn-danger" id="sr-route">' +
          esc(t('seqRecovery.route_button')) + '</button></div>';
    }
    if (nonMono.length) {
      html += '<h5>' + esc(t('seqRecovery.non_monotonic_heading')) + '</h5>' +
        '<table class="data-table"><thead><tr><th>' + esc(t('seqRecovery.col_did')) +
        '</th><th>' + esc(t('seqRecovery.col_seq')) + '</th><th>' +
        esc(t('seqRecovery.col_rev')) + '</th><th>' + esc(t('seqRecovery.col_prev_rev')) +
        '</th></tr></thead><tbody>' +
        nonMono.map((n) => '<tr><td><code>' + esc(n.did) + '</code></td><td>' +
          esc(String(n.seq)) + '</td><td><code>' + esc(n.rev) + '</code></td><td><code>' +
          esc(n.prevRev) + '</code></td></tr>').join('') +
        '</tbody></table>';
    }
    return html;
  }

  function renderRunning(p) {
    const card = el('sr-op');
    if (!card) return;
    card.innerHTML =
      '<h3>' + esc(t('seqRecovery.validating_heading')) + '</h3>' +
      '<dl class="kv-list">' +
      kv(t('seqRecovery.rows_scanned'), esc(String(p.rowsScanned || 0))) +
      kv(t('seqRecovery.started_at'), ts(p.startedAt, 'detail')) +
      '</dl>' +
      (p.cancelRequested
        ? '<p class="settings-help">' + esc(t('seqRecovery.cancel_pending')) + '</p>'
        : '<div class="form-actions"><button class="btn btn-secondary" id="sr-cancel">' +
          esc(t('seqRecovery.cancel_button')) + '</button></div>');
    const cb = el('sr-cancel');
    if (cb) cb.addEventListener('click', doCancel);
    clearPoll();
    poll = setTimeout(renderAfterTick, 2000);
  }

  async function renderAfterTick() {
    await renderOp();
    // When the run finishes, refresh the state card (head/last-validation).
    let p;
    try { p = await EP().getSequencerRecoveryProgress(); } catch (e) { return; }
    if (!p || !p.running) renderState();
  }

  async function doRun() {
    try {
      await EP().runSequencerRecovery({ operation: 'validate' });
      global.AuroraToast.success(t('seqRecovery.run_started'));
      renderOp();
    } catch (e) {
      if (e && e.status === 409) {
        global.AuroraToast.danger(t('seqRecovery.run_in_progress'));
        renderOp();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  // Route the malformed-event accounts to per-account repository rebuild
  // (§7.4.1). Destructive (fans out N rebuilds), so it goes through the typed
  // destructive-confirm + a required rationale, mirroring the repo-rebuild page.
  async function doRouteMalformed(affectedCount) {
    const res = await global.AuroraModal.destructiveConfirm({
      heading: t('seqRecovery.route_confirm_heading'),
      body: t('seqRecovery.route_confirm_body', { count: affectedCount }),
      rationaleRequired: true,
      typedConfirmGate: 'REBUILD',
      confirmLabel: t('seqRecovery.route_confirm_button'),
    });
    if (!res.confirmed) return;
    try {
      const out = await EP().runSequencerRecovery({
        operation: 'route_malformed',
        rationale: res.rationale || '',
      });
      const queued = (out && out.queued && out.queued.length) || 0;
      const skipped = (out && out.skipped && out.skipped.length) || 0;
      global.AuroraToast.success(t('seqRecovery.route_started', { queued: queued, skipped: skipped }));
      renderOp();
    } catch (e) {
      if (e && e.status === 400) {
        global.AuroraToast.danger(t('seqRecovery.route_unavailable'));
        renderOp();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  async function doCancel() {
    try {
      await EP().cancelSequencerRecovery();
      global.AuroraToast.success(t('seqRecovery.cancel_requested'));
      renderOp();
    } catch (e) {
      if (e && e.status === 409) {
        global.AuroraToast.danger(t('seqRecovery.cancel_none'));
        renderOp();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('opsSequencerRecovery', { mount: mount });
  }
})(window);
