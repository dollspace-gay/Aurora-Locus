// Mod Events page (route: #mod/events).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.3.6.

(function (global) {
  'use strict';

  let cursorStack = [];
  let nextCursor = null;
  let lastFilters = {};
  let subscription = null;

  async function mount({ container }) {
    container.innerHTML =
      '<header class="page-header">' +
      '  <div><h2>Events</h2><p class="page-subtitle">Cross-instance moderation event log via tools.aurora.moderator.queryEvents</p></div>' +
      '  <div id="events-rt-indicator" class="rt-indicator-slot"></div>' +
      '</header>' +
      '<div id="events-filter"></div>' +
      '<div id="events-table-container"></div>' +
      '<div id="events-pagination"></div>';
    cursorStack = [];
    nextCursor = null;
    lastFilters = {};
    if (global.AuroraFilterStrip) {
      global.AuroraFilterStrip.build({
        container: document.getElementById('events-filter'),
        filters: [
          { type: 'text', id: 'actor', placeholder: 'Filter by actor DID' },
          { type: 'text', id: 'subject', placeholder: 'Filter by subject DID' },
          { type: 'text', id: 'eventType', placeholder: 'Filter by event type' },
          { type: 'dateRange', id: 'when', label: 'Date range' },
        ],
        onApply: (vals) => { lastFilters = vals; cursorStack = []; nextCursor = null; refresh(null); },
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
    const indicator = document.getElementById('events-rt-indicator');
    subscription = global.AuroraSubscription.subscribe('subscribe-mod-events', {}, {
      onEvent: (event) => prependLive(event),
      onError: (e) => console.warn('subscribeModEvents error:', e),
    });
    if (indicator) global.AuroraSubscription.attachIndicator(indicator, subscription);
  }

  function prependLive(event) {
    if (cursorStack.length > 0) return;
    const tbody = document.querySelector('#events-table-container table.data-table tbody');
    if (!tbody) return;
    const tr = document.createElement('tr');
    tr.className = 'rt-fadein';
    const fmt = global.AuroraFormat;
    const actor = event.actorDid || '';
    let subj = '—';
    if (event.subjectDid) subj = global.AuroraEntityRef ? global.AuroraEntityRef.account(event.subjectDid) : 'repo: ' + event.subjectDid;
    else if (event.subjectUri) subj = global.AuroraEntityRef ? global.AuroraEntityRef.record(event.subjectUri) : 'record: ' + event.subjectUri;
    tr.innerHTML =
      '<td>' + esc(fmt ? fmt.date(event.createdAt, 'short') : event.createdAt) + '</td>' +
      '<td>' + esc(event.eventType) + '</td>' +
      '<td>' + (actor ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(actor) : esc(actor)) : '—') + '</td>' +
      '<td>' + subj + '</td>' +
      '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.event(event.id) : '#' + esc(event.id)) + '</td>';
    tbody.insertBefore(tr, tbody.firstChild);
    while (tbody.children.length > 100) tbody.removeChild(tbody.lastChild);
  }

  async function refresh(cursor) {
    const ep = global.AuroraEndpoints;
    const c = document.getElementById('events-table-container');
    if (!c || !ep) return;
    const params = { limit: 25 };
    if (lastFilters.actor) params.actor = lastFilters.actor;
    if (lastFilters.subject) params.subjectDid = lastFilters.subject;
    if (lastFilters.eventType) params.eventType = lastFilters.eventType;
    if (cursor) params.cursor = cursor;
    if (lastFilters.when && lastFilters.when.start) params.since = lastFilters.when.start.toISOString();
    if (lastFilters.when && lastFilters.when.end) params.until = lastFilters.when.end.toISOString();
    c.innerHTML = '<p class="empty-state">Loading…</p>';
    try {
      const data = await ep.moderator.queryEvents(params);
      const items = (data && data.items) || [];
      nextCursor = data && data.cursor;
      if (items.length === 0) {
        c.innerHTML = global.AuroraEmptyState
          ? global.AuroraEmptyState.render({ icon: 'inbox', primary: 'No events match these filters.' })
          : '<p class="empty-state">No events.</p>';
        renderPagination();
        return;
      }
      const fmt = global.AuroraFormat;
      let html = '<table class="data-table"><thead><tr>' +
                 '<th>When</th><th>Type</th><th>Actor</th><th>Subject</th><th>ID</th></tr></thead><tbody>';
      for (const e of items) {
        const actor = e.actorDid;
        let subj = '—';
        if (e.subject) subj = global.AuroraEntityRef ? global.AuroraEntityRef.fromSubject(e.subject) : esc(JSON.stringify(e.subject));
        html += '<tr>' +
                '<td>' + esc(fmt ? fmt.date(e.createdAt, 'short') : e.createdAt) + '</td>' +
                '<td>' + esc(e.eventType) + '</td>' +
                '<td>' + (actor ? (global.AuroraEntityRef ? global.AuroraEntityRef.account(actor, e.actorHandle) : esc(actor)) : '—') + '</td>' +
                '<td>' + subj + '</td>' +
                '<td>' + (global.AuroraEntityRef ? global.AuroraEntityRef.event(e.id) : '#' + esc(e.id)) + '</td>' +
                '</tr>';
      }
      html += '</tbody></table>';
      c.innerHTML = html;
      renderPagination();
    } catch (e) {
      c.innerHTML = '<p class="empty-state">Could not load events: ' + esc(e && e.message) + '</p>';
    }
  }

  function renderPagination() {
    const c = document.getElementById('events-pagination');
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
  if (global.AuroraRouter) global.AuroraRouter.register('modEvents', { mount: mount });
})(window);
