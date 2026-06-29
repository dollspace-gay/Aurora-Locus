// Repository rebuild page (Arc H §7.4.1). Route #ops/repo-rebuild[/:did],
// SuperAdmin only, visible in full + reduced modes (via the Operations
// domain gate). A per-account recovery surface: reconstruct an account's
// repository from its sequencer history and atomically swap it in.
//
// IA note (§7.4.1 left the route as an implementation detail): landed as a
// top-level Operations route with a DID-input affordance, plus a /:did
// variant so an account page can deep-link here pre-filled. Not nested under
// system-health — repository rebuild is a first-class recovery operation, so
// it gets its own Operations entry alongside the other escalation surfaces.
//
// Flow: enter/confirm DID → run pre-rebuild check (shallow; optional deep
// reconstruction+verification) → destructive-confirm (typed REBUILD +
// rationale) → poll getRebuildProgress through the walking/verifying/swapping
// phases → terminal summary. Cancellation is clean before the swap; the swap
// itself is atomic and uninterruptible. Consumes the four #286/#290 XRPCs.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }
  const t = (k, p) => (global.t ? global.t(k, p) : k);
  const EP = () => global.AuroraEndpoints.superadmin;

  // §10.4.3 — canonical timestamp rendering.
  function ts(value, context) {
    return global.AuroraTimestamp.render({ value: value, context: context });
  }

  let currentDid = '';
  let currentJobId = null;
  let pollTimer = null;
  let lastCheck = null;

  function clearPoll() {
    if (pollTimer) clearTimeout(pollTimer);
    pollTimer = null;
  }

  function el(id) { return document.getElementById(id); }

  function validDid(did) {
    return /^did:(plc|web):.+/.test(did || '');
  }

  function shortCid(cid) {
    if (!cid) return '—';
    return cid.length > 18 ? cid.slice(0, 10) + '…' + cid.slice(-6) : cid;
  }

  async function mount({ params, container }) {
    clearPoll();
    currentJobId = null;
    lastCheck = null;
    currentDid = params && params.did ? decodeURIComponent(params.did) : '';

    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#ops/accounts">' + esc(t('rebuild.crumb')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('rebuild.title')) +
      '</nav>' +
      '<header class="page-header"><div><h2>' + esc(t('rebuild.title')) +
      ' <span class="role-tag">SuperAdmin only</span></h2>' +
      '<p class="page-subtitle">' + esc(t('rebuild.subtitle')) + '</p></div></header>' +
      '<div class="settings-card" id="rb-did">' +
      '  <h3>' + esc(t('rebuild.did_heading')) + '</h3>' +
      '  <div class="form-row"><label for="rb-did-input">' + esc(t('rebuild.did_label')) +
        '</label>' +
      '    <input type="text" id="rb-did-input" class="form-input" placeholder="' +
        esc(t('rebuild.did_placeholder')) + '" value="' + esc(currentDid) + '"></div>' +
      '  <p class="settings-help">' + esc(t('rebuild.did_help')) + '</p>' +
      '  <label class="form-check"><input type="checkbox" id="rb-deep"> ' +
        esc(t('rebuild.deep_label')) + '</label>' +
      '  <p class="settings-help">' + esc(t('rebuild.deep_help')) + '</p>' +
      '  <div class="form-actions">' +
      '    <button class="btn btn-secondary" id="rb-check-btn">' +
        esc(t('rebuild.check_button')) + '</button></div>' +
      '</div>' +
      '<div class="settings-card" id="rb-check"><p class="settings-help">' +
        esc(t('rebuild.check_empty')) + '</p></div>' +
      '<div class="settings-card" id="rb-action"><p class="settings-help">' +
        esc(t('rebuild.action_need_check')) + '</p></div>';

    const btn = el('rb-check-btn');
    if (btn) btn.addEventListener('click', onRunCheck);
    const input = el('rb-did-input');
    if (input) {
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.preventDefault(); onRunCheck(); }
      });
    }

    // Arriving with a DID pre-fills + runs the shallow check immediately.
    if (currentDid) await runCheck(false);
    return { unmount: clearPoll };
  }

  function onRunCheck() {
    const deep = !!(el('rb-deep') && el('rb-deep').checked);
    runCheck(deep);
  }

  async function runCheck(deep) {
    const did = (el('rb-did-input') && el('rb-did-input').value.trim()) || currentDid;
    if (!validDid(did)) {
      global.AuroraToast.danger(t('rebuild.invalid_did'));
      return;
    }
    currentDid = did;
    const card = el('rb-check');
    if (!card) return;
    card.innerHTML = '<h3>' + esc(t('rebuild.check_heading')) + '</h3>' +
      global.AuroraSkeleton.lines(4);

    let r;
    try {
      r = await EP().preRebuildCheck(deep ? { did: did, deep: true } : { did: did });
    } catch (e) {
      lastCheck = null;
      if (e && e.status === 404) {
        card.innerHTML = '<h3>' + esc(t('rebuild.check_heading')) + '</h3>' +
          '<p class="settings-help">' + esc(t('rebuild.no_history')) + '</p>';
        renderActionDisabled(t('rebuild.no_history'));
        return;
      }
      card.innerHTML = '<h3>' + esc(t('rebuild.check_heading')) + '</h3><div id="rb-check-err"></div>';
      global.AuroraInlineError.mount(el('rb-check-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: () => runCheck(deep),
      });
      return;
    }
    lastCheck = r;
    renderCheck(r, deep);
  }

  function renderCheck(r, deep) {
    const card = el('rb-check');
    if (!card) return;
    const revRange = (r.firstRev || '—') + ' → ' + (r.headRev || '—');
    let html =
      '<h3>' + esc(t('rebuild.check_heading')) + '</h3>' +
      '<p class="settings-help">' + esc(t('rebuild.check_intro')) + '</p>' +
      '<dl class="kv-list">' +
      kv(t('rebuild.commit_count'), esc(String(r.commitCount != null ? r.commitCount : 0))) +
      kv(t('rebuild.record_count'), esc(String(r.recordCount != null ? r.recordCount : 0))) +
      kv(t('rebuild.creates'), esc(String(r.creates != null ? r.creates : 0))) +
      kv(t('rebuild.deletes'), esc(String(r.deletes != null ? r.deletes : 0))) +
      kv(t('rebuild.rev_range'), '<code>' + esc(revRange) + '</code>') +
      kv(t('rebuild.head_cid'), '<code>' + esc(shortCid(r.headCommitCid)) + '</code>');

    let blocked = null;
    if (deep) {
      if (r.deepVerified === true) {
        html += kv(t('rebuild.deep_verified'),
          global.AuroraStatusBadge.render('verified', t('rebuild.deep_ok')));
      } else {
        html += kv(t('rebuild.deep_verified'),
          global.AuroraStatusBadge.render('takedown', t('rebuild.deep_failed')));
        blocked = t('rebuild.deep_blocked', { message: r.deepError || '—' });
      }
      // Signing-key rotation history (#367). Surfaced on the deep preflight: an
      // account that has rotated keys carries multi-key commit history.
      if (r.rotatedKeysCount != null) {
        html += kv(t('rebuild.key_rotations'), esc(String(r.rotatedKeysCount)));
      } else if (r.keyHistoryError) {
        html += kv(t('rebuild.key_rotations'), esc(t('rebuild.key_history_unavailable')));
      }
      // History-aware verify (#368): did every commit verify against the key
      // valid at its rev? A failure is a significant signal — the account has a
      // verification anomaly across its key history.
      const hv = r.historyAwareVerifyResult;
      if (hv && hv.verified === true) {
        html += kv(t('rebuild.history_verify'),
          global.AuroraStatusBadge.render('verified',
            t('rebuild.history_verify_ok', { commits: hv.commitsVerified, keys: hv.keysUsed })));
      } else if (hv && hv.verified === false) {
        const f = hv.failure || {};
        html += kv(t('rebuild.history_verify'),
          global.AuroraStatusBadge.render('takedown',
            t('rebuild.history_verify_failed', { commit: shortCid(f.commitCid), reason: f.reason || '—' })));
        blocked = t('rebuild.history_verify_blocked');
      } else if (r.historyAwareVerifyError) {
        html += kv(t('rebuild.history_verify'),
          esc(t('rebuild.history_verify_unavailable', { error: r.historyAwareVerifyError })));
      }
    }
    html += '</dl>';
    if (blocked) {
      html += '<p class="settings-help" role="alert">' + esc(blocked) + '</p>';
    }
    card.innerHTML = html;

    if (blocked) renderActionDisabled(blocked);
    else renderActionButton();
  }

  function kv(label, valueHtml) {
    return '<div class="kv-row"><dt>' + esc(label) + '</dt><dd>' + valueHtml + '</dd></div>';
  }

  function renderActionDisabled(reason) {
    const card = el('rb-action');
    if (!card) return;
    card.innerHTML =
      '<h3>' + esc(t('rebuild.action_heading')) + '</h3>' +
      '<p class="settings-help">' + esc(t('rebuild.action_intro')) + '</p>' +
      '<div class="form-actions"><button class="btn btn-primary" disabled>' +
        esc(t('rebuild.action_button')) + '</button></div>' +
      '<p class="settings-help">' + esc(reason) + '</p>';
  }

  function renderActionButton() {
    const card = el('rb-action');
    if (!card) return;
    card.innerHTML =
      '<h3>' + esc(t('rebuild.action_heading')) + '</h3>' +
      '<p class="settings-help">' + esc(t('rebuild.action_intro')) + '</p>' +
      '<div class="form-actions"><button class="btn btn-primary" id="rb-go">' +
        esc(t('rebuild.action_button')) + '</button></div>';
    const btn = el('rb-go');
    if (btn) btn.addEventListener('click', doRebuild);
  }

  async function doRebuild() {
    const commits = lastCheck && lastCheck.commitCount != null ? lastCheck.commitCount : '?';
    const res = await global.AuroraModal.destructiveConfirm({
      heading: t('rebuild.confirm_heading'),
      body: t('rebuild.confirm_body', { did: currentDid, commits: String(commits) }),
      rationaleRequired: true,
      typedConfirmGate: 'REBUILD',
      confirmLabel: t('rebuild.confirm_button'),
    });
    if (!res.confirmed) return;
    try {
      const out = await EP().rebuildRepo({ did: currentDid, rationale: res.rationale || '' });
      currentJobId = out && out.jobId;
      global.AuroraToast.success(t('rebuild.started'));
      startProgress();
    } catch (e) {
      if (e && e.status === 409) {
        // Single-flight: a rebuild is already in flight for this DID. Progress
        // is keyed by job-id (no by-DID lookup), so recover the in-flight id
        // from the 409 message when present and resume polling it; otherwise
        // just inform the operator.
        global.AuroraToast.danger(t('rebuild.in_flight'));
        const m = /job ([0-9a-fA-F-]{36})/.exec((e && e.message) || '');
        if (m) { currentJobId = m[1]; startProgress(); }
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  function startProgress() {
    if (!currentJobId) return;
    renderProgress();
  }

  function schedule(ms) {
    clearPoll();
    pollTimer = setTimeout(renderProgress, ms);
  }

  function isTerminal(phase) {
    return phase === 'completed' || phase === 'failed' || phase === 'cancelled';
  }

  function phaseBadge(phase) {
    const variant = phase === 'completed' ? 'verified'
      : phase === 'failed' ? 'takedown'
      : phase === 'cancelled' ? 'deactivated'
      : 'pending';
    return global.AuroraStatusBadge.render(variant, t('rebuild.phase_' + phase));
  }

  async function renderProgress() {
    const card = el('rb-action');
    if (!card || !currentJobId) return;
    let p;
    try {
      p = await EP().getRebuildProgress(currentJobId);
    } catch (e) {
      // Transient (or the job id is unknown after a restart). Retry once on a
      // slow cadence rather than tearing down the surface.
      schedule(5000);
      return;
    }

    if (isTerminal(p.phase)) {
      renderTerminal(p);
      return;
    }

    const walking = p.phase === 'walking';
    const commitsCell = walking
      ? esc(String(p.commitsProcessed || 0)) + ' / ' + esc(String(p.commitsTotal || 0))
      : esc(String(p.commitsProcessed || 0));
    card.innerHTML =
      '<h3>' + esc(t('rebuild.progress_heading')) + '</h3>' +
      '<dl class="kv-list">' +
      kv(t('rebuild.phase'), phaseBadge(p.phase)) +
      kv(t('rebuild.commits_progress'), commitsCell) +
      kv(t('rebuild.records_written'), esc(String(p.recordsWritten || 0))) +
      kv(t('rebuild.head_before'), '<code>' + esc(shortCid(p.headCommitCidBefore)) + '</code>') +
      kv(t('rebuild.started_at'), ts(p.startedAt, 'detail')) +
      '</dl>' +
      (p.cancelRequested
        ? '<p class="settings-help">' + esc(t('rebuild.cancel_pending')) + '</p>'
        : '<div class="form-actions"><button class="btn btn-secondary" id="rb-cancel">' +
          esc(t('rebuild.cancel_button')) + '</button></div>' +
          '<p class="settings-help">' + esc(t('rebuild.cancel_note')) + '</p>');
    const cancelBtn = el('rb-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', doCancel);

    // §: 2s while walking (per-commit movement), 5s for verify/swap (coarse).
    schedule(walking ? 2000 : 5000);
  }

  function renderTerminal(p) {
    clearPoll();
    const card = el('rb-action');
    if (!card) return;
    if (p.phase === 'completed') {
      card.innerHTML =
        '<h3>' + esc(t('rebuild.action_heading')) + '</h3>' +
        '<p>' + phaseBadge('completed') + '</p>' +
        '<dl class="kv-list">' +
        kv(t('rebuild.records_written'), esc(String(p.recordsWritten || 0))) +
        kv(t('rebuild.head_before'), '<code>' + esc(shortCid(p.headCommitCidBefore)) + '</code>') +
        kv(t('rebuild.head_after'), '<code>' + esc(shortCid(p.headCommitCidAfter)) + '</code>') +
        '</dl>' +
        '<div class="form-actions"><button class="btn btn-secondary" id="rb-again">' +
          esc(t('rebuild.again_button')) + '</button></div>';
      const again = el('rb-again');
      if (again) again.addEventListener('click', () => { currentJobId = null; renderActionButton(); });
      // Audit pivot: the RepoRebuilt event lands queryable in the event log.
      // getRebuildProgress carries no event id, so the pivot is to the log
      // rather than a single entry (a direct entry deep-link would need the
      // progress payload to surface the audit id — a future enhancement).
      global.AuroraToast.success(t('rebuild.completed_toast'), {
        action: { label: t('rebuild.view_events'), href: '#mod/events' },
      });
    } else if (p.phase === 'cancelled') {
      card.innerHTML =
        '<h3>' + esc(t('rebuild.action_heading')) + '</h3>' +
        '<p>' + phaseBadge('cancelled') + '</p>' +
        '<p class="settings-help">' + esc(t('rebuild.cancelled_summary')) + '</p>' +
        '<div class="form-actions"><button class="btn btn-secondary" id="rb-again">' +
          esc(t('rebuild.again_button')) + '</button></div>';
      const again = el('rb-again');
      if (again) again.addEventListener('click', () => { currentJobId = null; renderActionButton(); });
    } else {
      // failed
      card.innerHTML =
        '<h3>' + esc(t('rebuild.action_heading')) + '</h3>' +
        '<p>' + phaseBadge('failed') + '</p><div id="rb-fail-err"></div>' +
        '<div class="form-actions"><button class="btn btn-secondary" id="rb-again">' +
          esc(t('rebuild.again_button')) + '</button></div>';
      global.AuroraInlineError.mount(el('rb-fail-err'), {
        message: t('rebuild.failed_summary', { message: p.error || '—' }),
      });
      const again = el('rb-again');
      if (again) again.addEventListener('click', () => { currentJobId = null; renderActionButton(); });
    }
  }

  async function doCancel() {
    if (!currentJobId) return;
    const res = await global.AuroraModal.form({
      heading: t('rebuild.cancel_confirm_heading'),
      body: t('rebuild.cancel_confirm_body') + ' ' + t('rebuild.cancel_note'),
      submitLabel: t('rebuild.cancel_button'),
      fields: [],
    });
    if (!res.submitted) return;
    try {
      await EP().cancelRebuild(currentJobId);
      global.AuroraToast.success(t('rebuild.cancel_requested'));
      renderProgress();
    } catch (e) {
      if (e && e.status === 409) {
        global.AuroraToast.danger(t('rebuild.cancel_none'));
        renderProgress();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('opsRepoRebuild', { mount: mount });
  }
})(window);
