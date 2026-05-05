// Route table for the Aurora Locus admin UI.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §4.3. Hash-based routing keeps
// no-build-step constraint; deep-linking works because the hash is
// part of the URL bookmark.
//
// Route shape:
//   { pattern: 'mod/events/:id', page: 'modEventDetail', requires: 'moderator' }
//
// Pattern matching: literal segments and :param placeholders. A
// trailing :rest captures the remainder for routes with structured
// IDs (e.g. URI-encoded record URIs).
//
// Section 12.8 legacy redirects are handled at router-mount time by
// matching the bare hash against the LEGACY_REDIRECTS map and
// rewriting the URL before normal route resolution.

(function (global) {
  'use strict';

  // Domain-grouped route table per §4.1.
  const ROUTES = [
    // Top-level
    { pattern: 'dashboard', page: 'dashboard' },
    { pattern: '', page: 'dashboard' }, // empty hash → dashboard

    // Moderation domain (Moderator+)
    { pattern: 'mod/queue', page: 'modQueue', requires: 'moderator' },
    { pattern: 'mod/reports', page: 'modReports', requires: 'moderator' },
    { pattern: 'mod/reports/:id', page: 'modReportDetail', requires: 'moderator' },
    { pattern: 'mod/appeals', page: 'modAppeals', requires: 'moderator' },
    { pattern: 'mod/appeals/:id', page: 'modAppealDetail', requires: 'moderator' },
    { pattern: 'mod/events', page: 'modEvents', requires: 'moderator' },
    { pattern: 'mod/events/:id', page: 'modEventDetail', requires: 'moderator' },
    { pattern: 'mod/audit', page: 'modAudit', requires: 'moderator' },
    { pattern: 'mod/audit/:id', page: 'modAuditDetail', requires: 'moderator' },

    // Operations domain
    { pattern: 'ops/dashboard', page: 'dashboard' }, // alias
    { pattern: 'ops/accounts', page: 'opsAccounts', requires: 'moderator' },
    { pattern: 'ops/accounts/:did', page: 'opsAccountDetail', requires: 'moderator' },
    { pattern: 'ops/records/:rest', page: 'opsRecordDetail', requires: 'moderator' },
    { pattern: 'ops/blobs/:cid', page: 'opsBlobDetail', requires: 'moderator' },
    { pattern: 'ops/invites', page: 'opsInvites', requires: 'admin' },
    { pattern: 'ops/invites/:code', page: 'opsInviteDetail', requires: 'admin' },
    { pattern: 'ops/sequencer', page: 'opsSequencer', requires: 'admin' },
    { pattern: 'ops/federation', page: 'opsFederation', requires: 'admin' },
    { pattern: 'ops/blob-ops', page: 'opsBlobOps', requires: 'admin' },
    { pattern: 'ops/rate-limits', page: 'opsRateLimits', requires: 'admin' },
    { pattern: 'ops/system-health', page: 'opsSystemHealth', requires: 'admin' },

    // Settings domain
    { pattern: 'settings', page: 'settingsGeneral' }, // default settings view
    { pattern: 'settings/general', page: 'settingsGeneral', requires: 'admin' },
    { pattern: 'settings/ui-modes', page: 'settingsUiModes' },
    { pattern: 'settings/roles', page: 'settingsRoles', requires: 'moderator' },
    { pattern: 'settings/roles/:role', page: 'settingsRolesMembers', requires: 'moderator' },
    { pattern: 'settings/capabilities', page: 'settingsCapabilities' },
  ];

  // Legacy hash redirects per §12.8. Operators with bookmarks pointing
  // at the old hash routes get auto-redirected to the new equivalent.
  const LEGACY_REDIRECTS = {
    'users': 'ops/accounts',
    'moderation': 'mod/queue',
    'reports': 'mod/reports',
    'invites': 'ops/invites',
    'settings': 'settings/general',
    'events': 'mod/events',
    'appeals': 'mod/appeals',
    'audit': 'mod/audit',
  };

  // Sidebar nav structure per §4.1.
  // Each item: { label, route, icon, badgeId?, requires? }
  const SIDEBAR = [
    {
      label: 'Dashboard',
      route: 'dashboard',
      icon: 'layout-dashboard',
    },
    {
      heading: 'Moderation',
      requires: 'moderator',
      items: [
        { label: 'Queue', route: 'mod/queue', icon: 'gavel', badgeId: 'mod-queue-count' },
        { label: 'Reports', route: 'mod/reports', icon: 'file-text', badgeId: 'reports-count' },
        { label: 'Appeals', route: 'mod/appeals', icon: 'scale' },
        { label: 'Events', route: 'mod/events', icon: 'shield-alert' },
        { label: 'Audit', route: 'mod/audit', icon: 'archive' },
      ],
    },
    {
      heading: 'Operations',
      items: [
        { label: 'Accounts', route: 'ops/accounts', icon: 'users', requires: 'moderator' },
        { label: 'Invites', route: 'ops/invites', icon: 'ticket', requires: 'admin' },
        { label: 'Sequencer', route: 'ops/sequencer', icon: 'activity', requires: 'admin' },
        { label: 'Federation', route: 'ops/federation', icon: 'network', requires: 'admin' },
        { label: 'Blob ops', route: 'ops/blob-ops', icon: 'image', requires: 'admin' },
        { label: 'Rate limits', route: 'ops/rate-limits', icon: 'gauge', requires: 'admin' },
        { label: 'System health', route: 'ops/system-health', icon: 'heart-pulse', requires: 'admin' },
      ],
    },
    {
      heading: 'Settings',
      items: [
        { label: 'General', route: 'settings/general', icon: 'settings', requires: 'admin' },
        { label: 'UI & modes', route: 'settings/ui-modes', icon: 'sliders' },
        { label: 'Roles', route: 'settings/roles', icon: 'key', requires: 'moderator' },
        { label: 'Capabilities', route: 'settings/capabilities', icon: 'plug' },
      ],
    },
  ];

  global.AuroraRoutes = {
    routes: ROUTES,
    legacyRedirects: LEGACY_REDIRECTS,
    sidebar: SIDEBAR,
  };
})(window);
