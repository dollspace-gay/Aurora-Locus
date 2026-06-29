# extension-point-author

A reference theme for **extension points** (Chapter 6 of `authoring-themes.md`).
It inherits `aurora-default` unchanged and declares three extension points so a
surface can opt into them via the runtime. Install it like any theme (drop the
directory under your themes path and restart); it validates and activates, but
its treatments only appear on surfaces that opt in.

## The three extension points

Each is declared in `manifest.json` (`providedExtensionPoints`) and defined in
`extensions.css` as a `.extension-<name>` class. A surface applies one only when
the active theme provides it:

```js
if (AuroraThemeRuntime.themeProvidesExtension('hero-treatment-cosmic')) {
  heroEl.classList.add('extension-hero-treatment-cosmic');
}
// else: the surface keeps its default treatment
```

### `hero-treatment-cosmic`

A deep-space gradient backdrop for a hero / landing section. Expects a
positioned, full-bleed container — the rule sets `position: relative` and paints
a layered radial+linear background, so apply it to the section wrapper.

```js
if (AuroraThemeRuntime.themeProvidesExtension('hero-treatment-cosmic')) {
  section.classList.add('extension-hero-treatment-cosmic');
}
```

### `accent-emphasis-glow`

An outer accent glow for an element the surface wants to emphasize — a primary
call-to-action, a highlighted card. Apply to the element itself.

```js
if (AuroraThemeRuntime.themeProvidesExtension('accent-emphasis-glow')) {
  ctaButton.classList.add('extension-accent-emphasis-glow');
}
```

### `nav-active-indicator`

An accent rail on the active navigation item. Expects the element to host a
`::before` pseudo-element (a nav link or button works). Apply to the active
item.

```js
if (AuroraThemeRuntime.themeProvidesExtension('nav-active-indicator')) {
  activeNavItem.classList.add('extension-nav-active-indicator');
}
```

## How this theme is put together

- **`manifest.json`** — `extends: "aurora-default"`, lists the three names in
  `providedExtensionPoints`, and points `files.extensions` at `extensions.css`.
- **`tokens.css`** — empty (inherits the full palette from `aurora-default`);
  present only because `files.tokens` is required.
- **`extensions.css`** — the three `.extension-<name>` definitions, each composed
  from inherited tokens (`var(--color-accent-primary)`, `color-mix(...)`) so they
  stay portable.

The substrate validates that every declared name has a matching
`.extension-<name>` definition; a declared-but-undefined point fails theme
validation. Operators can see this theme's extension points in the theme picker
(Configuration → Themes) without reading the CSS.

## Extending this further

A child theme that `extends: "extension-point-author"` *inherits* these three
extension points (extension points are additive across the chain) and can add
its own or redefine one of these by redeclaring the same `.extension-<name>`
rule.
