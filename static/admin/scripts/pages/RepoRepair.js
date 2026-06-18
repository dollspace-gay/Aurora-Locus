// Repository repair page (Arc H §7.4.3 / #293). Route #ops/repo-repair,
// SuperAdmin only, visible in full + reduced modes (via the Operations domain
// gate). The across-accounts "scrub": scan every account for repo-vs-sequencer
// inconsistencies, review the findings by severity, then bulk-repair all or a
// selected subset (each a per-account rebuild).
//
// Layout per §7.4.3: scan controls + scan results panel + repair action panel.
// Consumes the #291 scan XRPCs (scanReposForInconsistencies / getScanProgress /
// cancelScan / getRepoScanResults) and the #292 repair XRPCs (repairRepos /
// getBulkRepairProgress / cancelBulkRepair).

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

  let scanPoll = null;
  let repairPoll = null;
  const selected = new Set(); // dids checked for "repair selected"

  function clearPolls() {
    if (scanPoll) clearTimeout(scanPoll);
    if (repairPoll) clearTimeout(repairPoll);
    scanPoll = null;
    repairPoll = null;
  }

  function el(id) { return document.getElementById(id); }

  function shortCid(cid) {
    if (!cid) return '—';
    return cid.length > 16 ? cid.slice(0, 8) + '…' + cid.slice(-5) : cid;
  }

  function severityBadge(sev) {
    const variant = sev === 'high' ? 'takedown' : sev === 'medium' ? 'suspended' : 'pending';
    return global.AuroraStatusBadge.render(variant, t('repair.severity_' + sev));
  }

  async function mount({ container }) {
    clearPolls();
    selected.clear();
    container.innerHTML =
      '<nav class="breadcrumb" aria-label="Breadcrumb">' +
      '  <a href="#ops/accounts">' + esc(t('repair.crumb')) + '</a>' +
      '  <span class="breadcrumb-sep">›</span>' + esc(t('repair.title')) +
      '</nav>' +
      '<header class="page-header"><div><h2>' + esc(t('repair.title')) +
      ' <span class="role-tag">SuperAdmin only</span></h2>' +
      '<p class="page-subtitle">' + esc(t('repair.subtitle')) + '</p></div></header>' +
      '<div class="settings-card" id="rr-scan">' + global.AuroraSkeleton.lines(2) + '</div>' +
      '<div class="settings-card" id="rr-findings">' + global.AuroraSkeleton.lines(3) + '</div>' +
      '<div class="settings-card" id="rr-repair">' + global.AuroraSkeleton.lines(2) + '</div>';

    await renderScan();
    await renderFindings();
    await renderRepair();
    return { unmount: clearPolls };
  }

  // ---- Scan controls + progress ----

  async function renderScan() {
    const card = el('rr-scan');
    if (!card) return;
    let p;
    try { p = await EP().getScanProgress(); }
    catch (e) {
      card.innerHTML = '<h3>' + esc(t('repair.scan_heading')) + '</h3><div id="rr-scan-err"></div>';
      global.AuroraInlineError.mount(el('rr-scan-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: renderScan,
      });
      return;
    }
    if (p && p.running) {
      const cancelling = p.cancelRequested;
      card.innerHTML =
        '<h3>' + esc(t('repair.scanning_heading')) + '</h3>' +
        '<dl class="kv-list">' +
        kv(t('repair.accounts_scanned'), esc(String(p.accountsScanned || 0))) +
        kv(t('repair.findings_so_far'),
          esc(String((p.findingsHigh || 0) + (p.findingsMedium || 0) + (p.findingsLow || 0)))) +
        kv(t('repair.started_at'), ts(p.startedAt, 'detail')) +
        '</dl>' +
        (cancelling
          ? '<p class="settings-help">' + esc(t('repair.scan_cancel_pending')) + '</p>'
          : '<div class="form-actions"><button class="btn btn-secondary" id="rr-scan-cancel">' +
            esc(t('repair.scan_cancel_button')) + '</button></div>');
      const cb = el('rr-scan-cancel');
      if (cb) cb.addEventListener('click', doCancelScan);
      scanPoll = setTimeout(() => { renderScan(); }, 2000);
    } else {
      const last = p && p.lastOutcome
        ? '<p class="settings-help">' + esc(t('repair.scan_last', { outcome: p.lastOutcome })) + '</p>'
        : '';
      card.innerHTML =
        '<h3>' + esc(t('repair.scan_heading')) + '</h3>' +
        '<p class="settings-help">' + esc(t('repair.scan_explainer')) + '</p>' + last +
        '<div class="form-actions"><button class="btn btn-primary" id="rr-scan-run">' +
          esc(t('repair.scan_button')) + '</button></div>';
      const rb = el('rr-scan-run');
      if (rb) rb.addEventListener('click', doRunScan);
    }
  }

  async function doRunScan() {
    try {
      await EP().scanReposForInconsistencies();
      global.AuroraToast.success(t('repair.scan_started'));
      renderScan();
    } catch (e) {
      if (e && e.status === 409) {
        global.AuroraToast.danger(t('repair.scan_in_progress'));
        renderScan();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  async function doCancelScan() {
    try {
      await EP().cancelScan();
      global.AuroraToast.success(t('repair.scan_cancel_requested'));
      renderScan();
    } catch (e) {
      global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
    }
  }

  // ---- Findings panel ----

  async function renderFindings(severity) {
    const card = el('rr-findings');
    if (!card) return;
    let r;
    try { r = await EP().getRepoScanResults(severity ? { severity: severity } : {}); }
    catch (e) {
      card.innerHTML = '<h3>' + esc(t('repair.findings_heading')) + '</h3><div id="rr-find-err"></div>';
      global.AuroraInlineError.mount(el('rr-find-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: () => renderFindings(severity),
      });
      return;
    }
    const counts = r.counts || { high: 0, medium: 0, low: 0, total: 0 };
    const findings = r.findings || [];

    let html =
      '<h3>' + esc(t('repair.findings_heading')) + '</h3>' +
      '<p class="settings-help">' +
      esc(t('repair.findings_counts', {
        total: counts.total || 0, high: counts.high || 0,
        medium: counts.medium || 0, low: counts.low || 0,
      })) + '</p>';

    if ((counts.total || 0) === 0) {
      html += '<p class="settings-help">' + esc(t('repair.findings_empty')) + '</p>';
      card.innerHTML = html;
      return;
    }

    // Severity filter.
    const filters = ['', 'high', 'medium', 'low'];
    html += '<div class="filter-strip">' + filters.map((f) =>
      '<button class="btn btn-small' + ((severity || '') === f ? ' btn-active' : '') +
      '" data-sev="' + esc(f) + '">' +
      esc(f === '' ? t('repair.filter_all') : t('repair.severity_' + f)) + '</button>').join('') +
      '</div>';

    // Findings table with selection checkboxes.
    html += '<table class="data-table"><thead><tr>' +
      '<th></th><th>' + esc(t('repair.col_account')) + '</th>' +
      '<th>' + esc(t('repair.col_severity')) + '</th>' +
      '<th>' + esc(t('repair.col_detail')) + '</th>' +
      '<th>' + esc(t('repair.col_heads')) + '</th></tr></thead><tbody>';
    for (const f of findings) {
      const checked = selected.has(f.did) ? ' checked' : '';
      html += '<tr>' +
        '<td><input type="checkbox" class="rr-sel" data-did="' + esc(f.did) + '"' + checked + '></td>' +
        '<td><code>' + esc(f.did) + '</code></td>' +
        '<td>' + severityBadge(f.severity) + '</td>' +
        '<td>' + esc(f.detail || '') + '</td>' +
        '<td><code>' + esc(shortCid(f.liveHead)) + '</code> → <code>' +
          esc(shortCid(f.reconstructedHead)) + '</code></td>' +
        '</tr>';
    }
    html += '</tbody></table>';
    if (r.cursor) {
      html += '<div class="form-actions"><button class="btn btn-secondary" id="rr-more">' +
        esc(t('repair.load_more')) + '</button></div>';
    }
    card.innerHTML = html;

    // Wire filter buttons.
    card.querySelectorAll('[data-sev]').forEach((b) => {
      b.addEventListener('click', () => renderFindings(b.getAttribute('data-sev') || undefined));
    });
    // Wire selection checkboxes.
    card.querySelectorAll('.rr-sel').forEach((c) => {
      c.addEventListener('change', () => {
        const did = c.getAttribute('data-did');
        if (c.checked) selected.add(did); else selected.delete(did);
        renderRepairActions();
      });
    });
    const more = el('rr-more');
    if (more) more.addEventListener('click', () => loadMore(severity, r.cursor));
    renderRepairActions();
  }

  async function loadMore(severity, cursor) {
    // Simplest correct behaviour: append the next page by re-querying with the
    // cursor and inserting rows before the "load more" control. A full
    // re-render keeps the code simple at the cost of refetching page 1; given
    // findings sets are operator-scale, that's acceptable.
    let r;
    try {
      const params = { cursor: cursor };
      if (severity) params.severity = severity;
      r = await EP().getRepoScanResults(params);
    } catch (e) {
      global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      return;
    }
    const tbody = document.querySelector('#rr-findings table.data-table tbody');
    if (!tbody) return;
    for (const f of r.findings || []) {
      const tr = document.createElement('tr');
      const checked = selected.has(f.did) ? ' checked' : '';
      tr.innerHTML =
        '<td><input type="checkbox" class="rr-sel" data-did="' + esc(f.did) + '"' + checked + '></td>' +
        '<td><code>' + esc(f.did) + '</code></td>' +
        '<td>' + severityBadge(f.severity) + '</td>' +
        '<td>' + esc(f.detail || '') + '</td>' +
        '<td><code>' + esc(shortCid(f.liveHead)) + '</code> → <code>' +
          esc(shortCid(f.reconstructedHead)) + '</code></td>';
      tbody.appendChild(tr);
      tr.querySelector('.rr-sel').addEventListener('change', (ev) => {
        const did = ev.target.getAttribute('data-did');
        if (ev.target.checked) selected.add(did); else selected.delete(did);
        renderRepairActions();
      });
    }
    const more = el('rr-more');
    if (more) {
      if (r.cursor) more.replaceWith(makeMoreButton(severity, r.cursor));
      else more.remove();
    }
  }

  function makeMoreButton(severity, cursor) {
    const wrap = document.createElement('div');
    wrap.className = 'form-actions';
    const b = document.createElement('button');
    b.className = 'btn btn-secondary';
    b.id = 'rr-more';
    b.textContent = t('repair.load_more');
    b.addEventListener('click', () => loadMore(severity, cursor));
    wrap.appendChild(b);
    return wrap;
  }

  // ---- Repair actions + bulk progress ----

  async function renderRepair() {
    const card = el('rr-repair');
    if (!card) return;
    let p;
    try { p = await EP().getBulkRepairProgress(); }
    catch (e) {
      card.innerHTML = '<h3>' + esc(t('repair.repair_heading')) + '</h3><div id="rr-rep-err"></div>';
      global.AuroraInlineError.mount(el('rr-rep-err'), {
        message: t('common.error', { message: (e && e.message) || '' }),
        onRetry: renderRepair,
      });
      return;
    }
    if (p && p.running) {
      renderBulkProgress(p);
    } else {
      renderRepairActions(p);
    }
  }

  function renderRepairActions(lastProgress) {
    const card = el('rr-repair');
    if (!card) return;
    const n = selected.size;
    const last = lastProgress && lastProgress.lastOutcome
      ? '<p class="settings-help">' + esc(t('repair.repair_last', {
          outcome: lastProgress.lastOutcome,
          repaired: lastProgress.repaired || 0,
          skipped: lastProgress.skipped || 0,
          failed: lastProgress.failed || 0,
        })) + '</p>'
      : '';
    card.innerHTML =
      '<h3>' + esc(t('repair.repair_heading')) + '</h3>' +
      '<p class="settings-help">' + esc(t('repair.repair_explainer')) + '</p>' + last +
      '<div class="form-actions">' +
      '  <button class="btn btn-primary" id="rr-repair-sel"' + (n === 0 ? ' disabled' : '') + '>' +
        esc(t('repair.repair_selected', { count: n })) + '</button>' +
      '  <button class="btn btn-danger" id="rr-repair-all">' +
        esc(t('repair.repair_all')) + '</button>' +
      '</div>';
    const sel = el('rr-repair-sel');
    if (sel) sel.addEventListener('click', () => doRepair(false));
    const all = el('rr-repair-all');
    if (all) all.addEventListener('click', () => doRepair(true));
  }

  async function doRepair(all) {
    const count = all ? null : selected.size;
    if (!all && count === 0) return;
    const res = await global.AuroraModal.destructiveConfirm({
      heading: t('repair.confirm_heading'),
      body: all
        ? t('repair.confirm_body_all')
        : t('repair.confirm_body_selected', { count: count }),
      rationaleRequired: true,
      typedConfirmGate: 'REPAIR',
      confirmLabel: t('repair.confirm_button'),
    });
    if (!res.confirmed) return;
    const body = all
      ? { all: true, rationale: res.rationale || '' }
      : { dids: Array.from(selected), rationale: res.rationale || '' };
    try {
      const out = await EP().repairRepos(body);
      global.AuroraToast.success(t('repair.repair_started', { count: out.targetCount || 0 }));
      renderBulkProgress(null);
    } catch (e) {
      if (e && e.status === 409) {
        global.AuroraToast.danger(t('repair.repair_in_progress'));
        renderRepair();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  async function renderBulkProgress(known) {
    const card = el('rr-repair');
    if (!card) return;
    let p = known;
    if (!p) {
      try { p = await EP().getBulkRepairProgress(); }
      catch (e) { repairPoll = setTimeout(() => renderBulkProgress(null), 2000); return; }
    }
    if (!p.running) {
      renderRepairActions(p);
      renderFindings(); // refresh findings after a repair pass
      return;
    }
    card.innerHTML =
      '<h3>' + esc(t('repair.repairing_heading')) + '</h3>' +
      '<dl class="kv-list">' +
      kv(t('repair.progress'),
        esc(String(p.processed || 0)) + ' / ' + esc(String(p.targetsTotal || 0))) +
      kv(t('repair.repaired'), esc(String(p.repaired || 0))) +
      kv(t('repair.skipped'), esc(String(p.skipped || 0))) +
      kv(t('repair.failed'), esc(String(p.failed || 0))) +
      kv(t('repair.current'), p.currentDid ? '<code>' + esc(p.currentDid) + '</code>' : '—') +
      '</dl>' +
      (p.cancelRequested
        ? '<p class="settings-help">' + esc(t('repair.repair_cancel_pending')) + '</p>'
        : '<div class="form-actions"><button class="btn btn-secondary" id="rr-repair-cancel">' +
          esc(t('repair.repair_cancel_button')) + '</button></div>' +
          '<p class="settings-help">' + esc(t('repair.repair_cancel_note')) + '</p>');
    const cb = el('rr-repair-cancel');
    if (cb) cb.addEventListener('click', doCancelRepair);
    repairPoll = setTimeout(() => renderBulkProgress(null), 2000);
  }

  async function doCancelRepair() {
    try {
      await EP().cancelBulkRepair();
      global.AuroraToast.success(t('repair.repair_cancel_requested'));
      renderBulkProgress(null);
    } catch (e) {
      if (e && e.status === 409) {
        global.AuroraToast.danger(t('repair.repair_cancel_none'));
        renderRepair();
      } else {
        global.AuroraToast.danger(t('common.error', { message: (e && e.message) || '' }));
      }
    }
  }

  function kv(label, valueHtml) {
    return '<div class="kv-row"><dt>' + esc(label) + '</dt><dd>' + valueHtml + '</dd></div>';
  }

  if (global.AuroraRouter) {
    global.AuroraRouter.register('opsRepoRepair', { mount: mount });
  }
})(window);
