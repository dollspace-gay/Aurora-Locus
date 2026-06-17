// Blob detail page (route: #ops/blobs/:cid).
// Per docs/AURORA_ADMIN_UI_DESIGN.md §5.4.3.

(function (global) {
  'use strict';

  async function mount({ params, container }) {
    const cid = params && params.cid;
    container.innerHTML =
      '<nav class="breadcrumb"><a href="#ops/accounts">Operations</a> <span class="breadcrumb-sep">›</span> Blobs <span class="breadcrumb-sep">›</span> <code>' + esc(cid) + '</code></nav>' +
      '<header class="detail-header">' +
      '  <h2>Blob</h2>' +
      '  <p class="meta"><code>' + esc(cid) + '</code></p>' +
      '</header>' +
      '<div class="detail-layout">' +
      '  <div class="detail-primary" id="bd-primary">' + global.AuroraSkeleton.lines(4) + '</div>' +
      '  <aside class="detail-rail" aria-label="Context">' +
      '    <div class="rail-card" id="bd-owner"><h4>Owning account</h4>' + global.AuroraSkeleton.lines(3) + '</div>' +
      '    <div class="rail-card"><h4>References</h4><div id="bd-references">' + global.AuroraSkeleton.lines(2) + '</div></div>' +
      '  </aside>' +
      '</div>';
    await loadBlob(cid);
    return {};
  }

  async function loadBlob(cid) {
    const primary = document.getElementById('bd-primary');
    if (!primary) return;
    primary.innerHTML = global.AuroraSkeleton.lines(4);
    try {
      const data = await global.AuroraEndpoints.ops.listBlobs({ cid: cid, limit: 1 });
      const blobs = (data && data.blobs) || [];
      const blob = blobs[0];
      if (!blob) {
        primary.innerHTML = '<p class="empty-state">Blob not found on this PDS.</p>';
        return;
      }
      renderPrimary(blob, cid);
      renderOwner(blob.did);
      document.getElementById('bd-references').textContent =
        (blob.referenceCount != null ? String(blob.referenceCount) + ' references' : 'Reference count unavailable.');
    } catch (e) {
      global.AuroraErrorBoundary.mount(primary, {
        message: 'Could not load blob: ' + ((e && e.message) || ''),
        onRetry: function () { loadBlob(cid); },
      });
    }
  }

  function renderPrimary(blob, cid) {
    const fmt = global.AuroraFormat;
    const isImage = blob.mimeType && blob.mimeType.startsWith('image/');
    const previewSrc = isImage && blob.did
      ? '/xrpc/com.atproto.sync.getBlob?did=' + encodeURIComponent(blob.did) + '&cid=' + encodeURIComponent(cid)
      : null;
    const primary = document.getElementById('bd-primary');
    primary.innerHTML =
      '<div class="settings-card">' +
      '  <h3>Blob metadata</h3>' +
      '  <p><strong>CID:</strong> <code>' + esc(cid) + '</code></p>' +
      '  <p><strong>Mime:</strong> ' + esc(blob.mimeType || 'unknown') + '</p>' +
      '  <p><strong>Size:</strong> ' + esc(fmt ? fmt.bytes(blob.size) : (blob.size || '')) + '</p>' +
      '  <p><strong>Created:</strong> ' + global.AuroraTimestamp.render({ value: blob.createdAt, context: 'detail' }) + '</p>' +
      '</div>' +
      (previewSrc ? '<div class="settings-card" style="margin-top: 1rem;">' +
        '<h3>Preview</h3>' +
        '<img src="' + previewSrc + '" alt="" style="max-width: 100%; border-radius: 6px;">' +
        '</div>' : '') +
      '<div class="settings-card" style="margin-top: 1rem;">' +
      '  <h3>Actions</h3>' +
      '  <div id="bd-action-panel"></div>' +
      '</div>';
    if (typeof ActionPanel === 'function') {
      const session = global.AuroraSession;
      const panel = new ActionPanel({
        subject: { '$type': 'com.atproto.admin.defs#repoBlobRef', did: blob.did, cid: cid },
        availableActions: ['QuarantineBlob', 'RestoreBlob', 'DeleteBlob'],
        defaultAction: 'QuarantineBlob',
        requiresRationale: true,
        highImpactActions: ['DeleteBlob'],
        userRole: session ? session.role() : 'moderator',
      });
      panel.mount(document.getElementById('bd-action-panel'));
    }
  }

  function renderOwner(did) {
    const card = document.getElementById('bd-owner');
    if (!did) { card.innerHTML = '<h4>Owning account</h4><p class="empty-state">Unknown</p>'; return; }
    card.innerHTML = '<h4>Owning account</h4>' +
      (global.AuroraEntityRef ? global.AuroraEntityRef.account(did) : '<code>' + esc(did) + '</code>');
  }

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }
  if (global.AuroraRouter) global.AuroraRouter.register('opsBlobDetail', { mount: mount });
})(window);
