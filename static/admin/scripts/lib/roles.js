// Shared role helpers (§8.2.6 — extracted from the verbatim copies that lived
// in ConfigRoles.js and ConfigRolesMembers.js).
//
// `tierToRoleString` maps a UI role-tier slug (the plural form used in routes
// and tier headings — "moderators" / "administrators" / "superadmins") to the
// wire role string the role-management XRPC expects ("moderator" / "admin" /
// "superadmin"). Unknown tiers pass through unchanged so a future-additive
// tier doesn't silently mistranslate.

(function (global) {
  'use strict';

  function tierToRoleString(tier) {
    switch (tier) {
      case 'moderators':     return 'moderator';
      case 'administrators': return 'admin';
      case 'superadmins':    return 'superadmin';
      default:               return tier;
    }
  }

  global.AuroraRoles = { tierToRoleString: tierToRoleString };
})(window);
