// Configuration → Integration hooks (route: #configuration/integration-hooks).
//
// v0.9 Integration hooks Phase A (#350) — the declaration-without-execution
// surface. Operators declare WHERE + on WHAT events a future cycle would
// deliver webhooks; v0.9 does NOT execute them. The execution-status banner
// states this honestly (executionStatus.enabled is always false in v0.9).
// SuperAdmin-only (route-gated). The structural Layer 1 firewall (no HTTP
// client reachable from the declaration logic) lives in the hooks-core crate.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  let availableEventClasses = [];

  async function mount({ container }) {
    const session = global.AuroraSession;
    const isSuper = session && session.hasRole('superadmin');
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Integration hooks</nav>' +
      '<header class="page-header"><div><h2>Integration hooks</h2>' +
      '<p class="page-subtitle">Declare webhook endpoints for moderation/account events</p></div></header>' +
      '<div id="hooks-exec-banner" class="settings-help" style="padding:0.5rem; border-left:3px solid #888; background:#f3f4f6; margin-bottom:0.75rem;">Loading…</div>' +
      '<div class="settings-grid"><div class="settings-card">' +
      '  <h3>Declared hooks <span class="role-tag">SuperAdmin only</span></h3>' +
      '  <label><input type="checkbox" id="hooks-show-deleted"> Show deleted</label>' +
      '  <div id="hooks-list" style="margin:0.5rem 0;">Loading…</div>' +
      (isSuper ?
        '  <fieldset><legend id="hook-form-legend">Add hook</legend>' +
        '    <input type="hidden" id="hook-edit-id" value=""><input type="hidden" id="hook-edit-token" value="">' +
        '    <label style="display:block;">Name <input type="text" id="hook-name" style="width:100%;"></label>' +
        '    <label style="display:block;">URL (https only) <input type="text" id="hook-url" placeholder="https://…" style="width:100%;"></label>' +
        '    <p class="settings-help">URLs are stored in normalized form; host case-fold is automatic, and the fragment is stripped.</p>' +
        '    <fieldset style="margin-top:0.4rem;"><legend>Event classes</legend><div id="hook-event-classes"></div></fieldset>' +
        '    <label style="display:block;">Description <textarea id="hook-description" rows="2" style="width:100%;"></textarea></label>' +
        '    <label style="display:block;"><input type="checkbox" id="hook-enabled" checked> Enabled</label>' +
        '    <label style="display:block;">Rationale <textarea id="hook-rationale" rows="2" style="width:100%;"></textarea></label>' +
        '    <button type="button" class="btn-primary" id="hook-save">Save hook</button>' +
        '    <button type="button" id="hook-cancel" style="display:none;">Cancel edit</button>' +
        '  </fieldset>'
        : '  <p class="settings-help">SuperAdmin role required to manage integration hooks.</p>') +
      '</div></div>';

    await loadState();
    if (isSuper) {
      const save = document.getElementById('hook-save');
      if (save) save.addEventListener('click', saveHook);
      const cancel = document.getElementById('hook-cancel');
      if (cancel) cancel.addEventListener('click', resetForm);
    }
    const showDel = document.getElementById('hooks-show-deleted');
    if (showDel) showDel.addEventListener('change', loadHooks);
    return {};
  }

  function renderEventClassCheckboxes(selected) {
    const box = document.getElementById('hook-event-classes');
    if (!box) return;
    const sel = selected || [];
    box.innerHTML = availableEventClasses.map(function (c) {
      const checked = sel.indexOf(c) >= 0 ? ' checked' : '';
      return '<label style="display:block;"><input type="checkbox" class="hook-ec" value="' + esc(c) + '"' + checked + '> ' + esc(c) + '</label>';
    }).join('');
  }

  async function loadState() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.admin.getIntegrationHooksState();
      availableEventClasses = (data && data.availableEventClasses) || [];
      const banner = document.getElementById('hooks-exec-banner');
      const exec = (data && data.executionStatus) || {};
      if (banner) {
        banner.textContent = exec.enabled
          ? 'Hook execution is ENABLED.'
          : (exec.message || 'Hooks are declared but not yet executed; execution ships in a future cycle.');
      }
      renderEventClassCheckboxes([]);
      renderHooks((data && data.hooks) || []);
    } catch (e) {
      const banner = document.getElementById('hooks-exec-banner');
      if (banner) banner.textContent = 'Failed to load integration-hooks state.';
    }
  }

  async function loadHooks() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    const includeDeleted = !!(document.getElementById('hooks-show-deleted') || {}).checked;
    try {
      const data = await ep.admin.listHooks({ includeDeleted: includeDeleted });
      renderHooks((data && data.hooks) || []);
    } catch (e) { /* ignore */ }
  }

  // design-commit 24: truncate the URL inline (~80 chars) + full value in a
  // title tooltip (escaped for attribute context); host renders as-stored
  // (punycode never auto-decoded, design-commit 33).
  function truncatedUrl(url) {
    const short = url.length > 80 ? url.slice(0, 80) + '…' : url;
    return '<span title="' + esc(url.slice(0, 500)) + '">' + esc(short) + '</span>';
  }

  function renderHooks(hooks) {
    const list = document.getElementById('hooks-list');
    if (!list) return;
    if (!hooks.length) { list.textContent = 'No integration hooks declared.'; return; }
    list.innerHTML = hooks.map(function (h) {
      const deleted = h.deletedAt ? ' <em>(deleted)</em>' : '';
      const state = h.enabled ? 'enabled' : 'disabled';
      const classes = (h.eventClasses || []).map(esc).join(', ');
      return '<div class="hook-row" style="border-bottom:1px solid #ddd; padding:0.3rem 0;">' +
        '<strong>' + esc(h.name) + '</strong> → ' + truncatedUrl(h.url || '') + ' [' + esc(state) + ']' + deleted +
        '<br><span class="settings-help">' + classes + '</span>' +
        (h.deletedAt ? '' :
          ' <button type="button" class="hook-edit" data-id="' + esc(h.id) + '">Edit</button>' +
          ' <button type="button" class="hook-del" data-id="' + esc(h.id) + '">Delete</button>') +
        '</div>';
    }).join('');
    list.querySelectorAll('.hook-del').forEach(function (b) {
      b.addEventListener('click', function () { deleteHook(b.getAttribute('data-id')); });
    });
    list.querySelectorAll('.hook-edit').forEach(function (b) {
      b.addEventListener('click', function () { editHook(b.getAttribute('data-id'), hooks); });
    });
  }

  function selectedEventClasses() {
    const out = [];
    document.querySelectorAll('.hook-ec:checked').forEach(function (el) { out.push(el.value); });
    return out;
  }

  function resetForm() {
    document.getElementById('hook-edit-id').value = '';
    document.getElementById('hook-edit-token').value = '';
    document.getElementById('hook-form-legend').textContent = 'Add hook';
    document.getElementById('hook-cancel').style.display = 'none';
    document.getElementById('hook-name').value = '';
    document.getElementById('hook-url').value = '';
    document.getElementById('hook-description').value = '';
    document.getElementById('hook-rationale').value = '';
    document.getElementById('hook-enabled').checked = true;
    renderEventClassCheckboxes([]);
  }

  function editHook(id, hooks) {
    const h = hooks.filter(function (x) { return x.id === id; })[0];
    if (!h) return;
    document.getElementById('hook-edit-id').value = id;
    document.getElementById('hook-edit-token').value = h.lastModifiedAt || '';
    document.getElementById('hook-form-legend').textContent = 'Edit hook';
    document.getElementById('hook-cancel').style.display = '';
    document.getElementById('hook-name').value = h.name || '';
    document.getElementById('hook-url').value = h.url || '';
    document.getElementById('hook-description').value = h.description || '';
    document.getElementById('hook-rationale').value = h.rationale || '';
    document.getElementById('hook-enabled').checked = !!h.enabled;
    renderEventClassCheckboxes(h.eventClasses || []);
  }

  async function saveHook() {
    const ep = global.AuroraEndpoints;
    const name = document.getElementById('hook-name').value.trim();
    const url = document.getElementById('hook-url').value.trim();
    const eventClasses = selectedEventClasses();
    if (!name) { global.AuroraToast.warning('Name is required.'); return; }
    if (!url) { global.AuroraToast.warning('URL is required.'); return; }
    if (eventClasses.length === 0) { global.AuroraToast.warning('Select at least one event class.'); return; }
    const body = {
      name: name,
      url: url,
      eventClasses: eventClasses,
      description: document.getElementById('hook-description').value.trim() || null,
      enabled: document.getElementById('hook-enabled').checked,
      rationale: document.getElementById('hook-rationale').value.trim() || null,
    };
    const editId = document.getElementById('hook-edit-id').value;
    try {
      if (editId) {
        body.id = editId;
        body.expectedLastModifiedAt = document.getElementById('hook-edit-token').value;
        await ep.admin.editHook(body);
      } else {
        await ep.admin.createHook(body);
      }
      global.AuroraToast.success('Integration hook saved.');
      resetForm();
      await loadHooks();
    } catch (e) {
      global.AuroraToast.danger('Save failed: ' + (e && e.message ? e.message : ''));
    }
  }

  async function deleteHook(id) {
    const confirmResult = await global.AuroraModal.destructiveConfirm({
      heading: 'Delete integration hook',
      body: 'Soft-delete this hook? Deletion is one-way (no restore).',
      confirmLabel: 'Delete hook',
    });
    if (!confirmResult.confirmed) return;
    try {
      await global.AuroraEndpoints.admin.deleteHook({ id: id });
      global.AuroraToast.success('Hook deleted.');
      await loadHooks();
    } catch (e) {
      global.AuroraToast.danger('Delete failed: ' + (e && e.message ? e.message : ''));
    }
  }

  if (global.AuroraRouter) global.AuroraRouter.register('configIntegrationHooks', { mount: mount });
})(window);
