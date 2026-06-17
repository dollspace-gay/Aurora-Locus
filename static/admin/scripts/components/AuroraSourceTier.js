// AuroraSourceTier — the source-tier indicator primitive (§8.2.1).
//
// Settings reads return a `source` tier (Runtime / Default / File /
// RecoveryMode) alongside the value; this primitive renders that tier
// consistently across every Configuration settings card. Consolidates the
// `settingSourceSuffix` helper that had been copy-pasted into ConfigGeneral
// and ConfigUiModes (and referenced-but-deferred in ConfigRoles /
// ConfigRolesMembers) — now one primitive, applied uniformly.
//
//   AuroraSourceTier.suffix(source)  → "" | " (default)" | " (file)" | " (recovery override)"
//       The inline suffix appended after a setting's value (the established
//       Configuration-page convention; Runtime is the live tier, rendered
//       with no suffix so the common case stays uncluttered).
//   AuroraSourceTier.label(source)   → "Runtime" | "Default" | "File" | "Recovery override"
//   AuroraSourceTier.badge(source)   → a <span class="source-tier source-tier-*"> badge
//       (HTML string) for cards that prefer a discrete indicator over a suffix.

(function (global) {
  'use strict';

  function esc(s) {
    return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s);
  }

  function suffix(source) {
    switch (source) {
      case 'Runtime':      return '';
      case 'Default':      return ' (default)';
      case 'File':         return ' (file)';
      case 'RecoveryMode': return ' (recovery override)';
      default:             return '';
    }
  }

  function label(source) {
    switch (source) {
      case 'Runtime':      return 'Runtime';
      case 'Default':      return 'Default';
      case 'File':         return 'File';
      case 'RecoveryMode': return 'Recovery override';
      default:             return '';
    }
  }

  function badge(source) {
    const text = label(source);
    if (!text) return '';
    const cls = 'source-tier source-tier-' + String(source).toLowerCase();
    return '<span class="' + esc(cls) + '">' + esc(text) + '</span>';
  }

  global.AuroraSourceTier = { suffix: suffix, label: label, badge: badge };
})(window);
