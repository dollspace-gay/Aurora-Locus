// Sessions page (route: #configuration/sessions).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §8.1.7 — per-operator session
// management. Self-service for any operator (their own active sessions,
// current-session indicator, per-session revoke); SuperAdmin overview
// across all operators (filter by did; omit it for the all-operators view).
// Consumes the #273 listSessions / revokeSession XRPC.

(function (global) {
  'use strict';

  let cursorStack = [];
  let nextCursor = null;
  let filterDid = '';

  function T(key, params) { return global.t ? global.t(key, params) : key; }
  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }

  function isSuperadmin() {
    return !!(global.AuroraSession && global.AuroraSession.hasRole('superadmin'));
  }

  function ownDid() {
    const u = global.AuroraSession && global.AuroraSession.user();
    return (u && u.did) || localStorage.getItem('adminDid') || '';
  }

  async function mount({ container }) {
    cursorStack = [];
    nextCursor = null;
    filterDid = '';

    let html =
      '<nav class="breadcrumb"><a href="#configuration/general">' + esc(T('sessions.crumb')) +
      '</a> <span class="breadcrumb-sep">›</span> ' + esc(T('sessions.title')) + '</nav>' +
      '<header class="page-header"><div><h2>' + esc(T('sessions.title')) + '</h2>' +
      '<p class="page-subtitle">' + esc(isSuperadmin() ? T('sessions.subtitle_superadmin') : T('sessions.subtitle')) +
      '</p></div></header>';
    // SuperAdmin-only filter: a DID scopes to one operator; empty = all.
    if (isSuperadmin()) {
      html += '<div id="sessions-filter"></div>' +
        '<p class="filter-url-hint">' + esc(T('sessions.all_operators_hint')) + '</p>';
    }
    html += '<div id="sessions-table">' + global.AuroraSkeleton.lines(4) + '</div>' +
      '<div id="sessions-pagination"></div>';
    container.innerHTML = html;

    if (isSuperadmin() && global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('sessions-filter'),
        filters: [{ type: 'text', id: 'did', placeholder: T('sessions.filter_did_placeholder') }],
        initial: { did: '' },
        onApply: (vals) => {
          filterDid = (vals && vals.did) || '';
          cursorStack = [];
          nextCursor = null;
          refresh(null);
        },
      });
    }

    await refresh(null);
    return { unmount: () => { cursorStack = []; nextCursor = null; } };
  }

  async function refresh(cursor) {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('sessions-table');
    if (!c || !ep) return;
    c.innerHTML = global.AuroraSkeleton.lines(4);

    const params = { limit: 25 };
    if (filterDid) params.did = filterDid;
    if (cursor) params.cursor = cursor;

    let data;
    try {
      data = await ep.admin.listSessions(params);
    } catch (e) {
      global.AuroraInlineError.mount(c, {
        message: T('sessions.error') + (e && e.message ? ': ' + e.message : ''),
        onRetry: () => refresh(cursor),
      });
      return;
    }

    const sessions = (data && data.sessions) || [];
    nextCursor = (data && data.cursor) || null;
    renderTable(c, sessions);
    renderPagination();
  }

  function renderTable(c, sessions) {
    if (sessions.length === 0) {
      c.innerHTML = '<p class="empty-state">' + esc(T('sessions.empty')) + '</p>';
      return;
    }
    const ts = (v) => global.AuroraTimestamp.render({ value: v, context: 'activity' });
    const acct = (did) => global.AuroraEntityRef ? global.AuroraEntityRef.account(did) : '<code>' + esc(did) + '</code>';
    const rows = sessions.map((s) => {
      const status = s.isCurrent
        ? global.AuroraStatusBadge.render('current', T('sessions.status_current'))
        : global.AuroraStatusBadge.render('active', T('sessions.status_active'));
      const auditPivot = '<a href="#mod/audit?subject=' + encodeURIComponent(s.did) + '">' +
        esc(T('sessions.view_audit')) + '</a>';
      const revokeBtn = '<button class="btn-danger btn-sm" data-revoke-sid="' + esc(s.sid) +
        '" data-revoke-did="' + esc(s.did) + '" data-revoke-current="' + (s.isCurrent ? '1' : '') +
        '">' + esc(T('sessions.revoke')) + '</button>';
      return '<tr>' +
        '<td>' + acct(s.did) + '</td>' +
        '<td>' + esc(s.sourceIp || '—') + '</td>' +
        '<td>' + esc(s.userAgent || '—') + '</td>' +
        '<td>' + ts(s.createdAt) + '</td>' +
        '<td>' + ts(s.lastActiveAt) + '</td>' +
        '<td>' + status + '</td>' +
        '<td>' + revokeBtn + ' ' + auditPivot + '</td>' +
        '</tr>';
    }).join('');

    c.innerHTML = '<table class="data-table"><thead><tr>' +
      '<th>' + esc(T('sessions.col_operator')) + '</th>' +
      '<th>' + esc(T('sessions.col_source_ip')) + '</th>' +
      '<th>' + esc(T('sessions.col_user_agent')) + '</th>' +
      '<th>' + esc(T('sessions.col_created')) + '</th>' +
      '<th>' + esc(T('sessions.col_last_active')) + '</th>' +
      '<th>' + esc(T('sessions.col_status')) + '</th>' +
      '<th>' + esc(T('sessions.col_actions')) + '</th>' +
      '</tr></thead><tbody>' + rows + '</tbody></table>';

    c.querySelectorAll('[data-revoke-sid]').forEach((btn) => {
      btn.addEventListener('click', () => onRevoke(
        btn.getAttribute('data-revoke-sid'),
        btn.getAttribute('data-revoke-did'),
        btn.getAttribute('data-revoke-current') === '1',
      ));
    });
  }

  function renderPagination() {
    const c = document.getElementById('sessions-pagination');
    if (!c || !global.AuroraPagination) return;
    global.AuroraPagination.render({
      container: c,
      prevDisabled: cursorStack.length === 0,
      nextDisabled: !nextCursor,
      onPrev: () => {
        cursorStack.pop();
        refresh(cursorStack[cursorStack.length - 1] || null);
      },
      onNext: () => {
        if (nextCursor) { cursorStack.push(nextCursor); refresh(nextCursor); }
      },
    });
  }

  // Revoke dispatch:
  //   - another operator's session  → SuperAdmin-only; rationale required
  //     (the canonical destructive-confirm-with-rationale flow, mirroring
  //     the revokeRole surface). Security event.
  //   - own non-current session      → simple confirm, no rationale.
  //   - own CURRENT session          → confirm, then log out locally (the
  //     server already invalidated it; the next request would 401 anyway).
  async function onRevoke(sid, did, isCurrent) {
    const isSelf = did === ownDid();

    if (!isSelf) {
      const result = await global.AuroraModal.destructiveConfirm({
        heading: T('sessions.revoke_other_heading'),
        body: T('sessions.revoke_other_body', { did: did }),
        typedConfirmGate: 'REVOKE',
        rationaleRequired: true,
        confirmLabel: T('sessions.revoke_other_confirm'),
      });
      if (!result.confirmed) return;
      await doRevoke(sid, result.rationale, false);
      return;
    }

    const result = await global.AuroraModal.destructiveConfirm({
      heading: isCurrent ? T('sessions.revoke_current_heading') : T('sessions.revoke_self_heading'),
      body: isCurrent ? T('sessions.revoke_current_body') : T('sessions.revoke_self_body'),
      confirmLabel: T('sessions.revoke_self_confirm'),
    });
    if (!result.confirmed) return;
    await doRevoke(sid, null, isCurrent);
  }

  async function doRevoke(sid, rationale, logoutAfter) {
    const body = { sid: sid };
    if (rationale) body.rationale = rationale;
    try {
      const res = await global.AuroraEndpoints.admin.revokeSession(body);
      if (logoutAfter) {
        // The current session is gone; bounce to login.
        if (global.AuroraSession) global.AuroraSession.logout();
        return;
      }
      const auditEntryId = res && res.auditEntryId;
      global.AuroraToast.success(T('sessions.revoke_success'), auditEntryId ? {
        action: {
          label: T('sessions.view_audit'),
          href: '#mod/audit/' + encodeURIComponent(auditEntryId),
        },
      } : undefined);
      await refresh(cursorStack[cursorStack.length - 1] || null);
    } catch (e) {
      global.AuroraToast.danger(T('sessions.revoke_failed') + (e && e.message ? ': ' + e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configSessions', { mount: mount });
})(window);
