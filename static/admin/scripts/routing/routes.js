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

    // Configuration domain (was Settings — renamed in v0.9 per §5.5/§5.7.2).
    // The six policy/observability pages carry placeholder content from
    // Arc G until their backends land; Themes is owned by Arc B. Arc A
    // owns only the route definitions here.
    { pattern: 'configuration', page: 'configGeneral', requires: 'admin' }, // default configuration view
    { pattern: 'configuration/general', page: 'configGeneral', requires: 'admin' },
    { pattern: 'configuration/themes', page: 'configThemes', requires: 'admin' },
    { pattern: 'configuration/ui-modes', page: 'configUiModes' },
    { pattern: 'configuration/federation-policy', page: 'configFederationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/registration-policy', page: 'configRegistrationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/moderation-policy', page: 'configModerationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/kryphocron-policy', page: 'configKryphocronPolicy', requires: 'superadmin' },
    { pattern: 'configuration/integration-hooks', page: 'configIntegrationHooks', requires: 'superadmin' },
    { pattern: 'configuration/observability', page: 'configObservability', requires: 'superadmin' },
    { pattern: 'configuration/roles', page: 'configRoles', requires: 'moderator' },
    { pattern: 'configuration/roles/:role', page: 'configRolesMembers', requires: 'moderator' },
    { pattern: 'configuration/capabilities', page: 'configCapabilities' },

    // Kryphocron domain — full pages ship in Arc D (0.9.1). 0.9.0 carries
    // the group label plus a single stub child so the IA shape is stable
    // from 0.9.0 forward (§5.7.4 / kickoff item 1 default).
    { pattern: 'kryphocron', page: 'kryphocronStub', requires: 'moderator' },
  ];

  // Legacy hash redirects per §12.8. Operators with bookmarks pointing
  // at the old hash routes get auto-redirected to the new equivalent.
  const LEGACY_REDIRECTS = {
    'users': 'ops/accounts',
    'moderation': 'mod/queue',
    'reports': 'mod/reports',
    'invites': 'ops/invites',
    'events': 'mod/events',
    'appeals': 'mod/appeals',
    'audit': 'mod/audit',
    // v0.9 settings → configuration rename. Operators with v0.2-era
    // bookmarks at any settings.* hash land on the configuration
    // equivalent. applyLegacyRedirect (router.js) matches the full hash
    // path, so the multi-segment keys below resolve exactly.
    'settings': 'configuration/general',
    'settings/general': 'configuration/general',
    'settings/ui-modes': 'configuration/ui-modes',
    'settings/roles': 'configuration/roles',
    'settings/capabilities': 'configuration/capabilities',
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
      heading: 'Configuration',
      items: [
        { label: 'General', route: 'configuration/general', icon: 'settings', requires: 'admin' },
        { label: 'Themes', route: 'configuration/themes', icon: 'monitor', requires: 'admin' },
        { label: 'UI & modes', route: 'configuration/ui-modes', icon: 'sliders' },
        { label: 'Federation policy', route: 'configuration/federation-policy', icon: 'network', requires: 'superadmin' },
        { label: 'Registration policy', route: 'configuration/registration-policy', icon: 'inbox', requires: 'superadmin' },
        { label: 'Moderation policy', route: 'configuration/moderation-policy', icon: 'shield-check', requires: 'superadmin' },
        { label: 'Kryphocron policy', route: 'configuration/kryphocron-policy', icon: 'eye-off', requires: 'superadmin' },
        { label: 'Integration hooks', route: 'configuration/integration-hooks', icon: 'external-link', requires: 'superadmin' },
        { label: 'Observability', route: 'configuration/observability', icon: 'eye', requires: 'superadmin' },
        { label: 'Roles', route: 'configuration/roles', icon: 'key', requires: 'moderator' },
        { label: 'Capabilities', route: 'configuration/capabilities', icon: 'plug' },
      ],
    },
    {
      // Kryphocron domain pages ship in Arc D (0.9.1). 0.9.0 renders the
      // group label plus one "Available in 0.9.1" stub child so the IA
      // shape is stable from 0.9.0. Mode-aware visibility per §5.7.4 is
      // wired in the A-mode-gating pass; here the group is role-gated to
      // Moderator+ (the lowest tier that sees Kryphocron once Arc D lands).
      heading: 'Kryphocron',
      requires: 'moderator',
      items: [
        { label: 'Overview', route: 'kryphocron', icon: 'eye-off' },
      ],
    },
  ];

  global.AuroraRoutes = {
    routes: ROUTES,
    legacyRedirects: LEGACY_REDIRECTS,
    sidebar: SIDEBAR,
  };
})(window);
