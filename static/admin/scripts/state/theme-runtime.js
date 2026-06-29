// Theme extension-point runtime (§11.7 / #285).
//
// The operator-facing surface authors opt into. The design spec sketches it
// as `import { themeProvidesExtension } from '@aurora-locus/theme-runtime'`;
// the admin UI is a no-build vanilla-JS app, so the local idiom is the
// `AuroraThemeRuntime` global with the same `themeProvidesExtension(name)`
// function.
//
// A surface checks whether the active theme provides an extension point and
// applies the `.extension-<name>` treatment only when it does, falling back to
// a default otherwise:
//
//   if (AuroraThemeRuntime.themeProvidesExtension('hero-treatment-cosmic')) {
//     el.classList.add('extension-hero-treatment-cosmic');
//   }
//
// The check is synchronous against a cache of the active theme's EFFECTIVE
// extension points (own + inherited), loaded from /theme/active-extension-points
// at theme-load and refreshed on theme switch — the substrate resolves the
// inheritance chain server-side (themes::ThemeRegistry::resolve_extension_points).

(function (global) {
  'use strict';

  // null = not yet loaded; [] = loaded, theme provides none.
  let cache = null;
  let loadedFor = null;

  function urlFor(themeId) {
    const base = '/theme/active-extension-points';
    return themeId && themeId !== 'default'
      ? base + '?id=' + encodeURIComponent(themeId)
      : base;
  }

  // (Re)load the active theme's effective extension points into the cache.
  // Fail-soft: any error caches an empty set (surfaces fall back to defaults),
  // never throws — an extension-point lookup must not break a render.
  async function reload(themeId) {
    try {
      const res = await fetch(urlFor(themeId));
      const data = res.ok ? await res.json() : null;
      cache = data && Array.isArray(data.extensionPoints) ? data.extensionPoints : [];
    } catch (e) {
      cache = [];
    }
    loadedFor = themeId || 'default';
    return cache;
  }

  // Load once for `themeId` if not already cached for it.
  function ensureLoaded(themeId) {
    const key = themeId || 'default';
    if (cache !== null && loadedFor === key) return Promise.resolve(cache);
    return reload(themeId);
  }

  // Synchronous membership check. Returns false before the cache is populated
  // (call ensureLoaded() at boot) and for unknown points — the safe default
  // (the surface renders its non-extension fallback).
  function themeProvidesExtension(name) {
    return cache !== null && cache.indexOf(name) !== -1;
  }

  global.AuroraThemeRuntime = {
    themeProvidesExtension: themeProvidesExtension,
    ensureLoaded: ensureLoaded,
    reload: reload,
  };
})(window);
