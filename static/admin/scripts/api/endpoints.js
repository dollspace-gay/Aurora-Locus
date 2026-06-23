// Per-namespace endpoint helpers. Thin wrappers over AuroraClient that
// give pages a stable, named call-site instead of stringly-typed NSIDs
// scattered through the UI.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §12.3.3.

(function (global) {
  'use strict';

  const C = () => global.AuroraClient;

  // -------- com.atproto.* --------
  const atproto = {
    // Public server descriptor — real static config (inviteCodeRequired,
    // availableUserDomains, DID). Used by the Registration policy overview (#342).
    describeServer: () => C().get('com.atproto.server.describeServer'),
    // Public federation-scoped describe (#344) — Aurora-aware peer posture.
    describeFederationPosture: () => C().get('com.aurora.federation.describePosture'),
    listAccounts: (params) => C().get('com.atproto.admin.listAccounts', params || { limit: 100 }),
    getAccount: (did) => C().get('com.atproto.admin.getAccount', { did: did }),
    getAccountInfo: (did) => C().get('com.atproto.admin.getAccountInfo', { did: did }),
    searchAccounts: (params) => C().get('com.atproto.admin.searchAccounts', params || {}),
    listRoles: (params) => C().get('com.atproto.admin.listRoles', params || {}),
    getModerationQueue: (params) => C().get('com.atproto.admin.getModerationQueue', params || { limit: 50 }),
    listReports: (params) => C().get('com.atproto.admin.listReports', params || { limit: 50 }),
    listInviteCodes: (params) => C().get('com.atproto.admin.listInviteCodes', params || { limit: 100 }),
    getInviteCodes: (params) => C().get('com.atproto.admin.getInviteCodes', params || {}),
    createInviteCode: (body) => C().post('com.atproto.admin.createInviteCode', body),
    disableInviteCode: (body) => C().post('com.atproto.admin.disableInviteCode', body),
    disableInviteCodes: (body) => C().post('com.atproto.admin.disableInviteCodes', body),
    listRecentEvents: (params) => C().get('com.atproto.admin.listRecentEvents', params || { limit: 20 }),
    getRecord: (params) => C().get('com.atproto.repo.getRecord', params),
    getReport: (params) => C().get('tools.aurora.admin.getReport', params || {}),
    getSession: () => C().get('com.atproto.server.getSession'),
  };

  // -------- tools.aurora.admin.* --------
  const adminTools = {
    describeCapabilities: () => C().get('tools.aurora.describeCapabilities'),
    getQueueStats: () => C().get('tools.aurora.admin.getQueueStats'),
    // GET per the XRPC `query` convention (chainlink #118; route registered
    // `get(...)` in src/api/admin.rs). Params (timeRange/granularity/metrics)
    // ride the query string — `metrics` is a repeated key (client.get expands
    // arrays), which the handler's axum_extra Query requires.
    getModerationMetrics: (params) => C().get('tools.aurora.admin.getModerationMetrics', params),
    getAuditTrail: (params) => C().get('tools.aurora.admin.getAuditTrail', params || { limit: 25 }),
    triggerPasswordReset: (body) => C().post('tools.aurora.admin.triggerPasswordReset', body),
    exportAccountForensicRaw: (body) => C().postRaw('tools.aurora.admin.exportAccountForensic', body),
    getRuntimeSetting: (key) => C().get('tools.aurora.admin.getRuntimeSetting', { key: key }),
    setRuntimeSetting: (body) => C().post('tools.aurora.admin.setRuntimeSetting', body),
    emitEvent: (body) => C().post('tools.aurora.admin.emitEvent', body),
    // Per-operator session management (§8.1.7 / #273). listSessions: own
    // sessions (self-service) or, for SuperAdmin, a `did` for one operator
    // / omitted for all. revokeSession: force-logout a single sid.
    listSessions: (params) => C().get('tools.aurora.admin.listSessions', params || { limit: 25 }),
    revokeSession: (body) => C().post('tools.aurora.admin.revokeSession', body),
    // Bulk force-logout of every active session for one operator (SuperAdmin; #338).
    revokeOperatorSessions: (body) => C().post('tools.aurora.admin.revokeOperatorSessions', body || {}),
  };

  // -------- tools.aurora.moderator.* --------
  const moderatorTools = {
    queryEvents: (params) => C().get('tools.aurora.moderator.queryEvents', params || { limit: 25 }),
    getEvent: (id) => C().get('tools.aurora.moderator.getEvent', { id: id }),
    listAppeals: (params) => C().get('tools.aurora.moderator.listAppeals', params || { limit: 25 }),
    getAppeal: (id) => C().get('tools.aurora.moderator.getAppeal', { id: id }),
    getSubjectContext: (params) => C().get('tools.aurora.moderator.getSubjectContext', params || {}),
    getSubjectHistory: (params) => C().get('tools.aurora.moderator.getSubjectHistory', params || {}),
    resolveAppeal: (body) => C().post('tools.aurora.moderator.resolveAppeal', body),
  };

  // -------- tools.aurora.ops.* --------
  const opsTools = {
    getStats: () => C().get('tools.aurora.ops.getStats'),
    getInstanceMetrics: () => C().get('tools.aurora.ops.getInstanceMetrics'),
    getSystemHealth: () => C().get('tools.aurora.ops.getSystemHealth'),
    getFederationStatus: () => C().get('tools.aurora.ops.getFederationStatus'),
    // SuperAdmin full deployment-federation env view for the policy page (#344).
    getFederationPolicy: () => C().get('tools.aurora.ops.getFederationPolicy'),
    getVersionInfo: () => C().get('tools.aurora.ops.getVersionInfo'),
    listBlobs: (params) => C().get('tools.aurora.ops.listBlobs', params || {}),
    getBlobStatistics: () => C().get('tools.aurora.ops.getBlobStatistics'),
    getRelayConfig: () => C().get('tools.aurora.ops.getRelayConfig'),
    listKnownInstances: (params) => C().get('tools.aurora.ops.listKnownInstances', params || {}),
    getRateLimitConfig: () => C().get('tools.aurora.ops.getRateLimitConfig'),
    getRateLimitStatus: () => C().get('tools.aurora.ops.getRateLimitStatus'),
    getSequencerStatus: () => C().get('tools.aurora.ops.getSequencerStatus'),
    getDatabaseStatus: () => C().get('tools.aurora.ops.getDatabaseStatus'),
    getResourceUsage: () => C().get('tools.aurora.ops.getResourceUsage'),
    listBackgroundJobs: () => C().get('tools.aurora.ops.listBackgroundJobs'),
    getNonceStoreStatus: () => C().get('tools.aurora.ops.getNonceStoreStatus'),
    getValidationFailures: (params) => C().get('tools.aurora.ops.getValidationFailures', params || {}),
    // §11.10.2 — installed themes + their validation status (B-themes-page).
    listInstalledThemes: () => C().get('tools.aurora.ops.themes.listInstalled'),
    // Control POSTs (§8.1.5 fold-in — were stringly-typed AuroraClient.post
    // calls in the ops pages; registered here so every NSID has a named
    // wrapper). All take an empty body today.
    runBlobGC: () => C().post('tools.aurora.ops.runBlobGC', {}),
    pauseSequencer: () => C().post('tools.aurora.ops.pauseSequencer', {}),
    resumeSequencer: () => C().post('tools.aurora.ops.resumeSequencer', {}),
    resetSequencerCursor: () => C().post('tools.aurora.ops.resetSequencerCursor', {}),
    rebuildSequencer: () => C().post('tools.aurora.ops.rebuildSequencer', {}),
    triggerPdsDiscovery: () => C().post('tools.aurora.ops.triggerPdsDiscovery', {}),
    cleanupRateLimitState: () => C().post('tools.aurora.ops.cleanupRateLimitState', {}),
    runHealthChecks: () => C().post('tools.aurora.ops.runHealthChecks', {}),
    cleanupNonceStores: () => C().post('tools.aurora.ops.cleanupNonceStores', {}),
  };

  // -------- tools.aurora.ops.kryphocron.* (v0.9 Arc D, #225 backend) --------
  // The ten operator read XRPC backing the Kryphocron domain pages (§6.4,
  // §6.5). `triggerRotation` (#223) + the read cohort (#225). listAudiences /
  // getBlockCascadeImpact are per-account-filtered (the §6.5 drawer).
  opsTools.kryphocron = {
    getSubstrateInfo: () => C().get('tools.aurora.ops.kryphocron.getSubstrateInfo'),
    getTierStats: () => C().get('tools.aurora.ops.kryphocron.getTierStats'),
    getOracleActivity: () => C().get('tools.aurora.ops.kryphocron.getOracleActivity'),
    getRotationStatus: () => C().get('tools.aurora.ops.kryphocron.getRotationStatus'),
    getRotationProgress: () => C().get('tools.aurora.ops.kryphocron.getRotationProgress'),
    triggerRotation: (body) => C().post('tools.aurora.ops.kryphocron.triggerRotation', body || {}),
    cancelRotation: () => C().post('tools.aurora.ops.kryphocron.cancelRotation', {}),
    listRotations: () => C().get('tools.aurora.ops.kryphocron.listRotations'),
    getAudienceAggregate: () => C().get('tools.aurora.ops.kryphocron.getAudienceAggregate'),
    listAudiences: (account) =>
      C().get('tools.aurora.ops.kryphocron.listAudiences', { account: account }),
    getBlockCascadeImpact: (account) =>
      C().get('tools.aurora.ops.kryphocron.getBlockCascadeImpact', { account: account }),
    // Per-account overrides (#316, SuperAdmin) — Account Detail drawer.
    getAccountOverrides: (did) =>
      C().get('tools.aurora.ops.kryphocron.getAccountOverrides', { did: did }),
    setAccountOverride: (body) =>
      C().post('tools.aurora.ops.kryphocron.setAccountOverride', body || {}),
  };

  // -------- tools.aurora.superadmin.* --------
  const superadminTools = {
    grantRole: (body) => C().post('tools.aurora.superadmin.grantRole', body),
    revokeRole: (body) => C().post('tools.aurora.superadmin.revokeRole', body),
    // §5.5.4 Phase B (#346) — manual reviewer reassignment.
    assignReviewer: (body) => C().post('tools.aurora.superadmin.assignReviewer', body),
    // §5.5.4 Phase C (#347) — auto-label rule CRUD.
    createAutoLabelRule: (body) => C().post('tools.aurora.superadmin.createAutoLabelRule', body),
    editAutoLabelRule: (body) => C().post('tools.aurora.superadmin.editAutoLabelRule', body),
    deleteAutoLabelRule: (body) => C().post('tools.aurora.superadmin.deleteAutoLabelRule', body),
    listAutoLabelRules: (params) => C().get('tools.aurora.superadmin.listAutoLabelRules', params || {}),
    // §5.5.4 Phase D (#348) — escalation rule CRUD + de-escalation.
    createEscalationRule: (body) => C().post('tools.aurora.superadmin.createEscalationRule', body),
    editEscalationRule: (body) => C().post('tools.aurora.superadmin.editEscalationRule', body),
    deleteEscalationRule: (body) => C().post('tools.aurora.superadmin.deleteEscalationRule', body),
    listEscalationRules: (params) => C().get('tools.aurora.superadmin.listEscalationRules', params || {}),
    clearEscalation: (body) => C().post('tools.aurora.superadmin.clearEscalation', body),
    // Repository rebuild (§7.4.1 / #286 + #290). preRebuildCheck: shallow
    // metadata preflight ({did}), or full reconstruction+verification
    // ({did, deep:true}). rebuildRepo: start a rebuild ({did, rationale}) →
    // {jobId}. getRebuildProgress/cancelRebuild: poll/abort by job-id.
    preRebuildCheck: (params) => C().get('tools.aurora.superadmin.preRebuildCheck', params),
    rebuildRepo: (body) => C().post('tools.aurora.superadmin.rebuildRepo', body),
    getRebuildProgress: (jobId) =>
      C().get('tools.aurora.superadmin.getRebuildProgress', { jobId: jobId }),
    cancelRebuild: (jobId) =>
      C().post('tools.aurora.superadmin.cancelRebuild', { jobId: jobId }),
    // Bulk repository repair (§7.4.3 / #291 scan + #292 repair). scan*: start /
    // poll / cancel the across-accounts inconsistency scan + read findings.
    // repair*: start / poll / cancel the bulk repair over the findings.
    scanReposForInconsistencies: () =>
      C().post('tools.aurora.superadmin.scanReposForInconsistencies', {}),
    getScanProgress: () => C().get('tools.aurora.superadmin.getScanProgress'),
    cancelScan: () => C().post('tools.aurora.superadmin.cancelScan', {}),
    getRepoScanResults: (params) =>
      C().get('tools.aurora.superadmin.getRepoScanResults', params || {}),
    repairRepos: (body) => C().post('tools.aurora.superadmin.repairRepos', body),
    getBulkRepairProgress: () => C().get('tools.aurora.superadmin.getBulkRepairProgress'),
    cancelBulkRepair: () => C().post('tools.aurora.superadmin.cancelBulkRepair', {}),
    // Sequencer recovery (§7.4.2 / #294). options: current state + available
    // operations. run: dispatch one (v0.9: "validate", read-only deep
    // integrity validation). progress/cancel: poll/abort the in-flight op.
    sequencerRecoveryOptions: () => C().get('tools.aurora.superadmin.sequencerRecoveryOptions'),
    runSequencerRecovery: (body) => C().post('tools.aurora.superadmin.runSequencerRecovery', body),
    getSequencerRecoveryProgress: () =>
      C().get('tools.aurora.superadmin.getSequencerRecoveryProgress'),
    cancelSequencerRecovery: () => C().post('tools.aurora.superadmin.cancelSequencerRecovery', {}),
  };

  global.AuroraEndpoints = {
    atproto: atproto,
    admin: adminTools,
    moderator: moderatorTools,
    ops: opsTools,
    superadmin: superadminTools,
  };
})(window);
