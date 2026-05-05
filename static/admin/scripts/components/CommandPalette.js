// CommandPalette substrate primitive (substrate primitive 15) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.15.
//
// Global Cmd/Ctrl+K palette for navigation, subject lookup, and
// action invocation. Three result categories:
//   - NAVIGATE: registered routes
//   - SUBJECTS: account search (debounced 300ms)
//   - ACTIONS: registered command actions
//
// Recent items (last 10) persist in localStorage.

(function (global) {
  'use strict';

  const RECENT_KEY = 'aurora-admin-recent-commands';
  const RECENT_CAP = 10;
  let isOpen = false;
  let activeIdx = 0;
  let lastQueryToken = 0;
  let cachedAccountResults = [];

  // Build the static items list from the route table.
  function navItems() {
    if (!global.AuroraRoutes) return [];
    const out = [];
    for (const node of global.AuroraRoutes.sidebar) {
      if (node.route) {
        out.push({ id: 'nav.' + node.route, type: 'nav', label: 'Go to ' + node.label, route: node.route });
      }
      if (node.items) {
        for (const item of node.items) {
          out.push({ id: 'nav.' + item.route, type: 'nav', label: 'Go to ' + (node.heading ? node.heading + ' › ' : '') + item.label, route: item.route });
        }
      }
    }
    return out;
  }

  function actionItems() {
    return [
      { id: 'theme.cycle', type: 'action', label: 'Toggle theme (light → dark → system)', run: () => {
          const cur = global.AuroraSettings ? global.AuroraSettings.theme() : 'system';
          const next = cur === 'light' ? 'dark' : cur === 'dark' ? 'system' : 'light';
          if (global.AuroraSettings) global.AuroraSettings.setTheme(next);
        } },
      { id: 'logout', type: 'action', label: 'Log out', run: () => global.AuroraSession && global.AuroraSession.logout() },
    ];
  }

  function loadRecent() {
    try { return JSON.parse(localStorage.getItem(RECENT_KEY) || '[]'); }
    catch (e) { return []; }
  }

  function pushRecent(id) {
    let list = loadRecent().filter((x) => x !== id);
    list.unshift(id);
    if (list.length > RECENT_CAP) list = list.slice(0, RECENT_CAP);
    try { localStorage.setItem(RECENT_KEY, JSON.stringify(list)); } catch (e) {}
  }

  function open() {
    if (isOpen) return;
    isOpen = true;
    activeIdx = 0;
    cachedAccountResults = [];
    const root = ensureRoot();
    root.innerHTML = '';
    const modal = document.createElement('div');
    modal.className = 'modal command-palette-modal active';
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    modal.setAttribute('aria-label', 'Command palette');
    modal.innerHTML =
      '<div class="modal-header">' +
      '  <input type="text" id="cp-input" placeholder="Search anywhere…" aria-label="Command search" autofocus>' +
      '</div>' +
      '<div class="modal-body" style="padding-top: 0;">' +
      '  <div id="cp-results" role="listbox"></div>' +
      '</div>';
    root.appendChild(modal);

    const overlay = ensureOverlay();
    overlay.classList.add('active');
    overlay.addEventListener('click', overlayClick);
    document.addEventListener('keydown', escClose);

    const input = modal.querySelector('#cp-input');
    input.addEventListener('input', () => render(input.value));
    input.addEventListener('keydown', handleKey);
    setTimeout(() => input.focus(), 0);
    render('');
  }

  function close() {
    if (!isOpen) return;
    isOpen = false;
    const root = document.getElementById('palette-root');
    if (root) root.innerHTML = '';
    const overlay = document.getElementById('modal-overlay');
    if (overlay) {
      overlay.classList.remove('active');
      overlay.removeEventListener('click', overlayClick);
    }
    document.removeEventListener('keydown', escClose);
  }

  function ensureRoot() {
    let root = document.getElementById('palette-root');
    if (!root) {
      root = document.createElement('div');
      root.id = 'palette-root';
      document.body.appendChild(root);
    }
    return root;
  }

  function ensureOverlay() {
    let overlay = document.getElementById('modal-overlay');
    if (!overlay) {
      overlay = document.createElement('div');
      overlay.id = 'modal-overlay';
      overlay.className = 'modal-overlay';
      document.body.appendChild(overlay);
    }
    return overlay;
  }

  function overlayClick(e) {
    if (e.target.id === 'modal-overlay') close();
  }

  function escClose(e) {
    if (e.key === 'Escape') close();
  }

  async function fetchAccounts(query) {
    if (!query || query.length < 2 || !global.AuroraEndpoints) return [];
    const myToken = ++lastQueryToken;
    try {
      const data = await global.AuroraEndpoints.atproto.searchAccounts({ q: query, limit: 5 });
      if (myToken !== lastQueryToken) return [];
      const accs = (data && (data.accounts || data.users)) || [];
      return accs.map((a) => ({
        id: 'subj.' + a.did,
        type: 'subject',
        label: '@' + (a.handle || 'unknown') + ' — ' + a.did,
        route: 'ops/accounts/' + encodeURIComponent(a.did),
      }));
    } catch (e) { return []; }
  }

  async function render(q) {
    const list = document.getElementById('cp-results');
    if (!list) return;
    const lq = (q || '').toLowerCase().trim();
    const navs = navItems().filter((it) => !lq || it.label.toLowerCase().includes(lq));
    const acts = actionItems().filter((it) => !lq || it.label.toLowerCase().includes(lq));

    cachedAccountResults = await fetchAccounts(q);

    activeIdx = 0;
    let html = '';
    if (navs.length) {
      html += '<div class="cp-section-label">Navigate</div>';
      navs.forEach((it, i) => {
        html += '<div class="cp-item' + (i === 0 ? ' active' : '') + '" role="option" data-id="' + it.id + '">' +
                escHtml(it.label) + '</div>';
      });
    }
    if (cachedAccountResults.length) {
      html += '<div class="cp-section-label">Subjects</div>';
      cachedAccountResults.forEach((it) => {
        html += '<div class="cp-item" role="option" data-id="' + it.id + '">' + escHtml(it.label) + '</div>';
      });
    }
    if (acts.length) {
      html += '<div class="cp-section-label">Actions</div>';
      acts.forEach((it) => {
        html += '<div class="cp-item" role="option" data-id="' + it.id + '">' + escHtml(it.label) + '</div>';
      });
    }
    if (!html) html = '<div class="cp-item" style="opacity:0.6;">No results.</div>';
    list.innerHTML = html;
    list.querySelectorAll('.cp-item[data-id]').forEach((el, idx) => {
      el.addEventListener('click', () => activate(el.dataset.id));
      el.addEventListener('mouseenter', () => setActiveAt(idx));
    });
  }

  function escHtml(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s);
  }

  function allItems() {
    return [...navItems(), ...cachedAccountResults, ...actionItems()];
  }

  function findItem(id) {
    return allItems().find((it) => it.id === id);
  }

  function activate(id) {
    const it = findItem(id);
    if (!it) return;
    pushRecent(id);
    close();
    if (it.type === 'nav' || it.type === 'subject') {
      if (global.AuroraRouter) global.AuroraRouter.navigate(it.route);
    } else if (it.type === 'action' && typeof it.run === 'function') {
      it.run();
    }
  }

  function setActiveAt(idx) {
    const items = document.querySelectorAll('#cp-results .cp-item[data-id]');
    items.forEach((el, i) => el.classList.toggle('active', i === idx));
    activeIdx = idx;
  }

  function handleKey(e) {
    const items = document.querySelectorAll('#cp-results .cp-item[data-id]');
    if (items.length === 0) return;
    if (e.key === 'ArrowDown') { e.preventDefault(); setActiveAt((activeIdx + 1) % items.length); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setActiveAt((activeIdx - 1 + items.length) % items.length); }
    else if (e.key === 'Enter') {
      e.preventDefault();
      const target = items[activeIdx];
      if (target) activate(target.dataset.id);
    } else if (e.key === 'Escape') {
      e.preventDefault(); close();
    }
  }

  function start() {
    document.addEventListener('keydown', (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        if (isOpen) close(); else open();
      }
    });
  }

  global.AuroraCommandPalette = {
    open: open,
    close: close,
    start: start,
  };
})(window);
