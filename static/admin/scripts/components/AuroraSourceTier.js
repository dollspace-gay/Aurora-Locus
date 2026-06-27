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

  // v0.9 Federation runtime-mutability arc §4.1 (#401) — the tooltip that
  // disambiguates "Default" for the Pattern-1 federation fields: Default does NOT
  // mean the compiled fallback (e.g. `false`) — it means "no runtime override;
  // the consumer reads the env-seeded config value". `meta` is optional:
  //   { lastModified, lastModifiedBy, envVar, envValue }
  function tooltip(source, meta) {
    const m = meta || {};
    switch (source) {
      case 'Runtime':
        if (m.lastModifiedBy || m.lastModified) {
          return 'Set via admin UI' +
            (m.lastModifiedBy ? ' by ' + m.lastModifiedBy : '') +
            (m.lastModified ? ' at ' + m.lastModified : '') + '.';
        }
        return 'Set via the admin UI; overrides the environment configuration.';
      case 'Default':
        if (m.envVar && m.envValue != null && m.envValue !== '') {
          return 'Default: using ' + m.envVar + ' from the environment (' + m.envValue + ').';
        }
        if (m.envVar) {
          return 'Default: no runtime override set; reads ' + m.envVar + ' from the environment.';
        }
        return 'Default: no runtime override set; the field uses its environment-seeded value.';
      case 'File':
        return 'Override from the runtime.yaml file tier.';
      case 'RecoveryMode':
        return 'Recovery-mode override (set via the recovery environment).';
      default:
        return '';
    }
  }

  // `badge(source)` keeps the original signature; `badge(source, meta)` adds the
  // §4.1 disambiguating tooltip via a `title` attribute.
  function badge(source, meta) {
    const text = label(source);
    if (!text) return '';
    const cls = 'source-tier source-tier-' + String(source).toLowerCase();
    const tip = tooltip(source, meta);
    const titleAttr = tip ? ' title="' + esc(tip) + '"' : '';
    return '<span class="' + esc(cls) + '"' + titleAttr + '>' + esc(text) + '</span>';
  }

  global.AuroraSourceTier = { suffix: suffix, label: label, badge: badge, tooltip: tooltip };
})(window);
