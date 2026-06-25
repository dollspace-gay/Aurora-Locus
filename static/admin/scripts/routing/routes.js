// Route table for the Aurora-Locus admin.
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
    // Sequencer recovery (Arc H §7.4.2 / #295). SuperAdmin escalation surface
    // beyond the routine Sequencer controls — read-only deep integrity
    // validation. Nested under sequencer per the spec route.
    { pattern: 'ops/sequencer/recovery', page: 'opsSequencerRecovery', requires: 'superadmin' },
    { pattern: 'ops/federation', page: 'opsFederation', requires: 'admin' },
    { pattern: 'ops/blob-ops', page: 'opsBlobOps', requires: 'admin' },
    { pattern: 'ops/rate-limits', page: 'opsRateLimits', requires: 'admin' },
    { pattern: 'ops/system-health', page: 'opsSystemHealth', requires: 'admin' },
    // Repository rebuild (Arc H §7.4.1 / #288). SuperAdmin; per-account
    // recovery operation reconstructing a repo from sequencer history then
    // atomically swapping it in. Top-level Operations route with a DID-input
    // affordance, plus a :did variant for deep-linking pre-filled from an
    // account. full+reduced visibility follows the Operations domain gate.
    { pattern: 'ops/repo-rebuild', page: 'opsRepoRebuild', requires: 'superadmin' },
    { pattern: 'ops/repo-rebuild/:did', page: 'opsRepoRebuild', requires: 'superadmin' },
    // Bulk repository repair (Arc H §7.4.3 / #293). SuperAdmin; across-accounts
    // scan-then-repair. Scan controls + findings panel + repair actions.
    { pattern: 'ops/repo-repair', page: 'opsRepoRepair', requires: 'superadmin' },

    // Configuration domain (was Settings — renamed in v0.9 per §5.5/§5.7.2).
    // The six policy/observability pages carry placeholder content from
    // Arc G until their backends land; Themes is owned by Arc B. Arc A
    // owns only the route definitions here.
    { pattern: 'configuration', page: 'configGeneral', requires: 'admin' }, // default configuration view
    { pattern: 'configuration/general', page: 'configGeneral', requires: 'admin' },
    { pattern: 'configuration/ui-modes', page: 'configUiModes' },
    { pattern: 'configuration/federation-policy', page: 'configFederationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/registration-policy', page: 'configRegistrationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/moderation-policy', page: 'configModerationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/kryphocron-policy', page: 'configKryphocronPolicy', requires: 'superadmin' },
    // Key-rotation arc B2 (#373 / §4.6). SuperAdmin; the operator-supplied-keys
    // feature gate for per-account signing-key rotation. Own page (not folded
    // into Kryphocron policy) — signing-key rotation ≠ the at-rest codec layer.
    { pattern: 'configuration/key-rotation-policy', page: 'configKeyRotationPolicy', requires: 'superadmin' },
    { pattern: 'configuration/integration-hooks', page: 'configIntegrationHooks', requires: 'superadmin' },
    { pattern: 'configuration/observability', page: 'configObservability', requires: 'superadmin' },
    { pattern: 'configuration/roles', page: 'configRoles', requires: 'moderator' },
    { pattern: 'configuration/roles/:role', page: 'configRolesMembers', requires: 'moderator' },
    { pattern: 'configuration/capabilities', page: 'configCapabilities' },
    // Per-operator session management (§8.1.7 / Arc E 0.9.3). `moderator`
    // = any authenticated operator: the page is self-service (your own
    // sessions); the SuperAdmin all-operators overview is gated inside the
    // page + the listSessions handler. Lives under Configuration (always
    // mode-visible) so self-service works in every moderation mode; the
    // primary entry point is the sidebar-footer "Sessions" link.
    { pattern: 'configuration/sessions', page: 'configSessions', requires: 'moderator' },
    // Recovery mode status (Arc H / §7.3.2). SuperAdmin; read-only status +
    // documented env+restart enter/exit procedure (the substrate's recovery
    // entry is startup-scoped by design — no runtime controls). Under
    // Configuration so it stays reachable in every moderation mode
    // (recovery is operationally important even in disabled mode).
    { pattern: 'configuration/recovery-mode', page: 'configRecoveryMode', requires: 'superadmin' },

    // Kryphocron domain (Arc D / 0.9.1) — the four domain pages (§6.4) plus
    // the Laquna rotation-history sub-page (§6.4.2.1). Bare `kryphocron`
    // resolves to Overview (§6.4.1's "bare #kryphocron redirects to
    // overview"). Overview / Audiences / Tier activity are Moderator+
    // (observability); Laquna + its history sub-page are Admin+ (the
    // rotation operator action). The kryphocron *domain* visibility (the
    // §5.7.4 role × moderation-mode matrix in domainMinRole below) was
    // already wired Moderator+ in Arc A; these per-route `requires` realise
    // the in-domain Admin+ gate on the Laquna surfaces.
    { pattern: 'kryphocron', page: 'kryphocronOverview', requires: 'moderator' },
    { pattern: 'kryphocron/overview', page: 'kryphocronOverview', requires: 'moderator' },
    { pattern: 'kryphocron/laquna', page: 'kryphocronLaquna', requires: 'admin' },
    { pattern: 'kryphocron/laquna/history', page: 'kryphocronLaqunaHistory', requires: 'admin' },
    { pattern: 'kryphocron/audiences', page: 'kryphocronAudiences', requires: 'moderator' },
    { pattern: 'kryphocron/tier-activity', page: 'kryphocronTierActivity', requires: 'moderator' },
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
    // #322 — the standalone Themes page was folded into UI & modes; old
    // bookmarks (v0.9 pre-fold or the v0.2-era settings hash) land there.
    'settings/themes': 'configuration/ui-modes',
    'configuration/themes': 'configuration/ui-modes',
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
      // The bell badge lives on the group label (§5.8.2): combined count
      // of open reports + pending appeals, and the label itself links to
      // the Queue. Visibility (Moderator+ in full mode only) follows from
      // the domain gate — the Moderation domain renders only in full mode.
      heading: 'Moderation',
      route: 'mod/queue',
      badgeId: 'mod-attention-count',
      items: [
        { label: 'Queue', route: 'mod/queue', icon: 'gavel' },
        { label: 'Reports', route: 'mod/reports', icon: 'file-text' },
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
        { label: 'Repository rebuild', route: 'ops/repo-rebuild', icon: 'server', requires: 'superadmin' },
        { label: 'Repository repair', route: 'ops/repo-repair', icon: 'shield-check', requires: 'superadmin' },
        { label: 'Sequencer recovery', route: 'ops/sequencer/recovery', icon: 'activity', requires: 'superadmin' },
      ],
    },
    {
      heading: 'Configuration',
      items: [
        { label: 'General', route: 'configuration/general', icon: 'settings', requires: 'admin' },
        { label: 'UI & modes', route: 'configuration/ui-modes', icon: 'sliders' },
        { label: 'Recovery mode', route: 'configuration/recovery-mode', icon: 'life-buoy', requires: 'superadmin' },
        { label: 'Federation policy', route: 'configuration/federation-policy', icon: 'network', requires: 'superadmin' },
        { label: 'Registration policy', route: 'configuration/registration-policy', icon: 'inbox', requires: 'superadmin' },
        { label: 'Moderation policy', route: 'configuration/moderation-policy', icon: 'shield-check', requires: 'superadmin' },
        { label: 'Kryphocron policy', route: 'configuration/kryphocron-policy', icon: 'eye-off', requires: 'superadmin' },
        { label: 'Key rotation policy', route: 'configuration/key-rotation-policy', icon: 'refresh-cw', requires: 'superadmin' },
        { label: 'Integration hooks', route: 'configuration/integration-hooks', icon: 'external-link', requires: 'superadmin' },
        { label: 'Observability', route: 'configuration/observability', icon: 'eye', requires: 'superadmin' },
        { label: 'Roles', route: 'configuration/roles', icon: 'key', requires: 'moderator' },
        { label: 'Capabilities', route: 'configuration/capabilities', icon: 'plug' },
      ],
    },
    {
      // Kryphocron domain (Arc D / 0.9.1) — the four §6.4 domain pages.
      // Mode-aware visibility per §5.7.4 comes from the domain gate
      // (domainMinRole('kryphocron') = Moderator+ in full/reduced, hidden
      // in disabled). Per-item `requires` realises the in-domain Admin+
      // gate: a Moderator sees Overview / Audience aggregate / Tier
      // activity but not Laquna. The rotation-history sub-page is reached
      // from the Laquna page, not the sidebar.
      heading: 'Kryphocron',
      items: [
        { label: 'Overview', route: 'kryphocron/overview', icon: 'eye-off' },
        { label: 'Laquna', route: 'kryphocron/laquna', icon: 'refresh-cw', requires: 'admin' },
        { label: 'Audience aggregate', route: 'kryphocron/audiences', icon: 'users' },
        { label: 'Tier activity', route: 'kryphocron/tier-activity', icon: 'bar-chart-2' },
      ],
    },
  ];

  // --- Domain visibility: the §5.7.4 role × moderation-mode matrix ---
  //
  // Single source of truth for which top-level domains are reachable for
  // a given (role, mode), consumed by both the sidebar (app.js) and the
  // router dispatch gate (router.js). Encodes this matrix exactly:
  //
  //   role \ mode    full                              reduced                      disabled
  //   Moderator      Dash, Mod, Config, Kryph          Dash, Config, Kryph          Config
  //   Admin          Dash, Mod, Ops, Config, Kryph     Dash, Ops, Config, Kryph     Config
  //   SuperAdmin     all five                          all except Mod               Config
  //
  // Configuration is always visible (it holds the mode toggle). Within a
  // visible domain, per-item `requires` still applies — that realises the
  // "Configuration (limited)" cells (a Moderator sees UI & modes / Roles /
  // Capabilities but not the SuperAdmin policy pages).
  const ROLE_ORDER = { moderator: 1, admin: 2, superadmin: 3 };

  // Map a route pattern (or page-less hash prefix) to its top-level domain.
  function domainForPattern(pattern) {
    const p = String(pattern || '');
    if (p === '' || p === 'dashboard' || p === 'ops/dashboard') return 'dashboard';
    if (p === 'mod' || p.indexOf('mod/') === 0) return 'moderation';
    if (p === 'ops' || p.indexOf('ops/') === 0) return 'operations';
    if (p === 'configuration' || p.indexOf('configuration/') === 0) return 'configuration';
    if (p === 'kryphocron' || p.indexOf('kryphocron/') === 0) return 'kryphocron';
    return 'dashboard';
  }

  // Minimum role tier that can see `domain` in `mode`, or null if the
  // domain is hidden in that mode for every role.
  function domainMinRole(domain, mode) {
    switch (domain) {
      case 'configuration':
        return 'moderator'; // always visible
      case 'dashboard':
        return (mode === 'disabled') ? null : 'moderator';
      case 'moderation':
        return (mode === 'full') ? 'moderator' : null;
      case 'operations':
        return (mode === 'disabled') ? null : 'admin';
      case 'kryphocron':
        return (mode === 'disabled') ? null : 'moderator';
      default:
        return 'moderator';
    }
  }

  // Sidebar visibility: applies BOTH the mode rule and the domain's role
  // tier — e.g. a Moderator never sees the (Admin+) Operations group.
  function domainVisible(domain, role, mode) {
    const min = domainMinRole(domain, mode);
    if (!min) return false;
    return (ROLE_ORDER[role] || 0) >= (ROLE_ORDER[min] || 0);
  }

  // Router reachability: applies ONLY the mode rule, not the domain's
  // sidebar role tier. A route's own `requires` is the authoritative role
  // gate, and some routes sit below their domain's sidebar tier — e.g.
  // ops/accounts is Moderator-reachable (via the command palette / report
  // pivots) even though the Operations *group* is Admin+ in the sidebar.
  // Gating route dispatch on the sidebar tier would wrongly 404 those.
  function domainModeAllowed(domain, mode) {
    return domainMinRole(domain, mode) !== null;
  }

  global.AuroraRoutes = {
    routes: ROUTES,
    legacyRedirects: LEGACY_REDIRECTS,
    sidebar: SIDEBAR,
    domainForPattern: domainForPattern,
    domainVisible: domainVisible,
    domainModeAllowed: domainModeAllowed,
  };
})(window);
