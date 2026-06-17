// Capabilities page (route: #configuration/capabilities) — hosts capabilities
// probe + version info. Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.6.6.

(function (global) {
  'use strict';

  async function mount({ container }) {
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#configuration/general">Configuration</a> <span class="breadcrumb-sep">›</span> Capabilities</nav>' +
      '<header class="page-header">' +
      '  <div><h2>Capabilities</h2><p class="page-subtitle">Capabilities and build information</p></div>' +
      '  <div class="header-actions">' +
      '    <button class="btn-secondary" id="srv-refresh">Refresh</button>' +
      '    <button class="btn-secondary" id="srv-copy">Copy raw JSON</button>' +
      '  </div>' +
      '</header>' +
      '<div class="settings-card" id="srv-caps"><h3>Capabilities</h3><p class="empty-state">Loading…</p></div>' +
      '<div class="settings-card" id="srv-build" style="margin-top: 1rem;"><h3>Build information</h3><p class="empty-state">Loading…</p></div>';
    document.getElementById('srv-refresh').addEventListener('click', refresh);
    document.getElementById('srv-copy').addEventListener('click', copyRaw);
    await refresh();
    return {};
  }

  let lastCaps = null;

  async function refresh() {
    const ep = global.AuroraEndpoints;
    if (!ep) return;
    try {
      const data = await ep.admin.describeCapabilities();
      lastCaps = data;
      renderCaps(data);
    } catch (e) {
      document.getElementById('srv-caps').innerHTML =
        '<h3>Capabilities</h3><p class="empty-state">Could not probe capabilities.</p>';
    }
    try {
      const v = await ep.ops.getVersionInfo();
      const fmt = global.AuroraFormat;
      document.getElementById('srv-build').innerHTML =
        '<h3>Build information</h3>' +
        '<dl class="capability-meta">' +
        '  <p><strong>Version:</strong> ' + esc(v.version || '—') + '</p>' +
        '  <p><strong>Commit:</strong> <code>' + esc(v.commit || '—') + '</code></p>' +
        '  <p><strong>Built:</strong> ' + global.AuroraTimestamp.render({ value: v.builtAt, context: 'detail' }) + '</p>' +
        '  <p><strong>Rust:</strong> ' + esc(v.rustVersion || '—') + '</p>' +
        '</dl>';
    } catch (e) {
      document.getElementById('srv-build').innerHTML =
        '<h3>Build information</h3><p class="empty-state">Unavailable.</p>';
    }
  }

  function renderCaps(data) {
    const known = global.AuroraCapabilities ? Array.from(global.AuroraCapabilities._knownCapabilities) : [];
    const advertised = new Set();
    if (data && Array.isArray(data.extensions)) {
      for (const ext of data.extensions) if (ext && ext.name) advertised.add(ext.name);
    }
    if (data && data.families) {
      // Inferred capabilities from families table (mirrors capabilities.js logic)
      const adminFamily = data.families['tools.aurora.admin'] || [];
      if (Array.isArray(adminFamily) && adminFamily.includes('emitEvent')) advertised.add('mod-events-emit-v1');
      if (Array.isArray(adminFamily) && adminFamily.some((n) => n.startsWith('batch'))) advertised.add('batch-takedown-v1');
    }
    let listHtml = '<ul class="capability-list">';
    for (const c of known) {
      const yes = advertised.has(c);
      const ic = global.AuroraIcons ? global.AuroraIcons.render(yes ? 'check-circle' : 'x-circle', 14) : (yes ? '✓' : '✗');
      listHtml += '<li class="' + (yes ? 'cap-yes' : 'cap-no') + '">' + ic + ' ' + esc(c) + '</li>';
    }
    listHtml += '</ul>';
    document.getElementById('srv-caps').innerHTML =
      '<h3>Capabilities</h3>' +
      listHtml +
      '<div class="capability-meta">' +
      '  <p>Version: ' + esc(data && data.version || '—') + '</p>' +
      '  <p>Implementation: ' + esc(data && data.implementation || '—') + '</p>' +
      '</div>';
  }

  async function copyRaw() {
    if (!lastCaps) return;
    try {
      await navigator.clipboard.writeText(JSON.stringify(lastCaps, null, 2));
      global.AuroraToast.success('Capabilities JSON copied.');
    } catch (e) {
      global.AuroraToast.warning('Clipboard write failed; open browser console for raw value.');
      console.log(JSON.stringify(lastCaps, null, 2));
    }
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) {
    global.AuroraRouter.register('configCapabilities', { mount: mount });
  }
})(window);
