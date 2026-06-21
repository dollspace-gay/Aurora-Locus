# Authoring Aurora-Locus themes

This guide is for **theme authors** — operators who want to restyle the
Aurora-Locus admin by forking a bundled theme, building a theme from scratch,
or tuning one for accessibility. It assumes you have Aurora-Locus running and
know basic CSS. It does *not* cover substrate internals (see the v0.9 design
doc) or general CSS (see [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS)).

The theming substrate ships in Aurora-Locus 0.9, and this guide covers it in
full — all ten chapters, including extension points (Chapter 6) and
forward-compatibility (Chapter 10). Chapter 10's version-by-version migration log
is a living section that gains an entry each time a real substrate change ships;
the rules it states hold today.

---

## Chapter 1 — Quick start: clone and customize

The five-minute path: ship a theme that changes the accent color and inherits
everything else.

### 1.1 Locate your theme directory

Aurora-Locus enumerates themes from `<data-dir>/themes/` at startup, where
`<data-dir>` is your `PDS_DATA_DIRECTORY`. If you launched with
`PDS_DATA_DIRECTORY=/var/lib/aurora-locus`, your operator theme directory is
`/var/lib/aurora-locus/themes/`. Create it if it doesn't exist:

```bash
mkdir -p "$PDS_DATA_DIRECTORY/themes"
```

The four bundled themes (`aurora-default`, `aurora-light`, `aurora-dark`,
`aurora-stack-classic`) ship inside the binary's static assets and are always
available as inheritance parents — you don't need to copy them to customize.

### 1.2 Copy a bundled theme as a starting point

The bundled themes live under `static/admin/themes/` in the Aurora-Locus source
tree. Copy `aurora-default` (the sober root) into your operator directory:

```bash
cp -r static/admin/themes/aurora-default "$PDS_DATA_DIRECTORY/themes/my-theme"
```

### 1.3 Edit `manifest.json`

Each theme directory has a `manifest.json`. Change three fields and **leave the
rest alone for now**:

```json
{
  "schemaVersion": "1.0",
  "themeId": "my-theme",
  "themeName": "My Theme",
  "themeVersion": "1.0.0",
  "substrateVersion": "1.0",
  "extends": "aurora-default",
  "files": { "tokens": "tokens.css", "effects": "effects.css" }
}
```

`themeId` **must equal the directory name** (`my-theme`). Set `extends` to
`aurora-default` so you inherit the full 28-token contract and the effect
library.

> The directory you copied from `aurora-default` is the root theme, so its
> manifest has *no* `extends`. Your fork is *not* the root — add
> `"extends": "aurora-default"`. (A non-root theme with no `extends` fails
> validation: `theme.chain.orphan`.) You can also delete the copied `effects.css`
> entirely and drop `effects` from `files` — you'll inherit aurora-default's
> effect classes. See Chapter 8.

### 1.4 Edit `tokens.css`

Change one token and delete the rest (you inherit them):

```css
:root {
  --color-accent-primary: #2f9e7e; /* your accent */
}
```

### 1.5 Restart Aurora-Locus

The substrate enumerates and validates themes **at startup only** — there is no
hot-reload in 0.9.0. Restart the process. Watch the startup log for the theme
summary; a validation failure is logged with a `theme.*` reason (Chapter 9).

### 1.6 Activate your theme

- **Deployment-wide:** Configuration → Themes → *Set as deployment default* on
  your theme's card (SuperAdmin; a rationale is recorded in the audit chain).
- **Just for you:** Configuration → UI & modes → pick your theme from the
  theme picker.

### 1.7 What you just did

Your theme defines exactly one token and inherits the other 27 required tokens,
all optional tokens, and the entire effect library from `aurora-default`. That
is the substrate's core idea: **override what you want, inherit the rest.**
Continue with Chapter 2 to see what's available.

---

## Chapter 2 — The theming substrate at a glance

A theme is a directory with a manifest and one or more CSS files. The substrate
gives you three layers of customization, in increasing power and effort:

- **Tokens** are *values* — a color, a font family, a duration. `tokens.css`
  declares CSS custom properties (`--color-accent-primary: #2f9e7e;`). Every
  admin surface reads tokens, so changing a token reshapes the whole UI. There
  are **28 required tokens** (Chapter 4); you may add your own.
- **Effect classes** are *compositions* — a gradient, a glow, a glass blur.
  `effects.css` declares CSS classes (`.effect-frosted-glass { … }`) that
  surfaces apply. The substrate ships a baseline library; you override
  individual classes (Chapter 5).
- **Extension points** are *theme-specific surfaces* — opt-in regions a theme
  lights up. These ship in 0.9.1 (Chapter 6).

### 2.1 Inheritance

Every theme `extends` another, and every chain terminates at the root,
`aurora-default`. The substrate resolves a theme by walking the chain root→leaf
and concatenating each level's CSS, so a leaf's declarations override its
ancestors'. Chains are at most **4 levels deep**.

```
aurora-default            (root — the 28-token contract + effect library)
  └── aurora-dark         (extends default — deeper surfaces, richer effects)
        └── my-theme      (extends aurora-dark — your overrides)
```

### 2.2 Required vs optional

The substrate is opinionated about exactly one thing: **accessibility**. It
requires the 28 tokens (so no surface renders against an undefined value), four
effect classes (so focus and elevation always work), and that the
contrast-bearing token pairs meet WCAG 2.2 thresholds (Chapter 7). Everything
else — your palette, your typography, your decorative effects — is yours.

### 2.3 The contrast contract's verification flow

```
your theme ─▶ resolve tokens through the chain ─▶ for each of the 13
  contrast pairs, resolve both sides to a concrete color ─▶ compute the
  WCAG 2.2 ratio ─▶ meets threshold?  ──yes─▶ pass
                                       ──no──▶ theme rejected (fail-closed)
```

A theme that fails contrast is **rejected**, not silently degraded — it won't
appear as selectable, and its card on the Themes page shows the failure. This is
deliberate: an inaccessible theme never reaches an operator.

---

## Chapter 3 — The manifest file

`manifest.json` (one per theme directory) is JSON. Field-by-field:

### 3.1 Required fields

| Field | Type | Notes |
|---|---|---|
| `schemaVersion` | string | Manifest schema version. Use `"1.0"`. |
| `themeId` | string | Must equal the directory name. Lowercase-kebab by convention. |
| `themeName` | string | Human-readable; shown in the picker and Themes page. |
| `substrateVersion` | string | The substrate version your theme targets. `"1.0"` for 0.9.0. Must not exceed the running substrate's version. |
| `extends` | string | The parent theme id. Required for every theme **except** `aurora-default` (the root, which omits it). Must resolve to an installed theme; the chain must terminate at `aurora-default`. |
| `files` | object | At minimum `{ "tokens": "tokens.css" }`. |

### 3.2 Optional fields

| Field | Type | Use case |
|---|---|---|
| `themeVersion` | string | Your theme's own version (e.g. `"1.0.0"`). Shown on the Themes page. |
| `themeAuthor` | string | Attribution. |
| `themeDescription` | string | One-line summary on the Themes page. |
| `files.effects` | string | Your effect-class CSS file (Chapter 5). Omit to inherit. |
| `files.extensions` | string | Extension-point CSS (0.9.1; Chapter 6). |
| `files.preview` | string | A preview image path for the Themes page. |
| `providedExtensionPoints` | array | Declared extension points (0.9.1; Chapter 6). |
| `metadata` | object | Free-form; the substrate ignores it. |

### 3.3 A minimal manifest

```json
{
  "schemaVersion": "1.0",
  "themeId": "my-theme",
  "themeName": "My Theme",
  "substrateVersion": "1.0",
  "extends": "aurora-default",
  "files": { "tokens": "tokens.css" }
}
```

### 3.4 The substrate-version compatibility rule

`substrateVersion` declares the minimum substrate your theme expects. The
running substrate accepts a theme whose `substrateVersion` is **less than or
equal to** its own. A theme targeting a *newer* substrate than is running is
rejected (`theme.substrate.version.future`) — it may use tokens or classes this
version doesn't provide. For 0.9.0 the substrate version is `1.0`.

### 3.5 A note on the filename

The substrate reads `manifest.json`. (Some design material refers to
`theme.manifest.json`; the on-disk name the substrate enumerates is
`manifest.json`.) The file must be valid JSON; a parse failure means the theme
is skipped at enumeration (it never enters the registry) — check the startup log
if a theme doesn't appear.

---

## Chapter 4 — Tokens: what's required and what's optional

### 4.1 The 28 required tokens

Every theme must define these, directly or by inheritance. They group into
surfaces, text, accent, status, borders, typography, sizing, and motion:

```css
:root {
  /* Surfaces — page/card/nested backgrounds + modal scrim */
  --color-surface-primary:   /* main page background */;
  --color-surface-secondary: /* cards, panels, drawers */;
  --color-surface-tertiary:  /* nested cards, sub-panels */;
  --color-surface-overlay:   /* modal/dialog backdrop (usually translucent) */;

  /* Text — on the surfaces above */
  --color-text-primary:   /* body text */;
  --color-text-secondary: /* secondary text, labels */;
  --color-text-tertiary:  /* captions, placeholders */;
  --color-text-inverted:  /* text on accent-filled surfaces (e.g. buttons) */;

  /* Accent — primary interactive color + its states */
  --color-accent-primary:        /* buttons, links, focus ring */;
  --color-accent-primary-hover:  ;
  --color-accent-primary-active: ;
  --color-accent-secondary:      /* secondary interactive */;

  /* Status */
  --color-status-success: ;
  --color-status-warning: ;
  --color-status-danger:  ;
  --color-status-info:    ;

  /* Borders */
  --color-border-primary:   /* between cards/sections */;
  --color-border-secondary: /* subtle within-card dividers */;
  --color-border-focus:     /* focus ring — usually var(--color-accent-primary) */;

  /* Typography */
  --font-family-sans:    ;
  --font-family-mono:    ;
  --font-family-display: ;

  /* Sizing */
  --font-size-base: /* e.g. 14px */;
  --space-unit:     /* base spacing step, e.g. 4px */;

  /* Motion */
  --motion-duration-fast:    /* e.g. 120ms */;
  --motion-duration-medium:  ;
  --motion-duration-slow:    ;
  --motion-easing-standard:  /* an easing function */;
}
```

A theme missing any of these (in itself or its chain) is rejected:
`theme.tokens.required.missing: <token>`.

### 4.2 Optional substrate tokens

`aurora-default` also defines structural tokens the UI uses that aren't in the
required contract — spacing scale steps (`--space-1` … `--space-12`), radii
(`--radius-sm`, `--radius-md`, `--radius-full`), and font-size steps. You
inherit these from `aurora-default` automatically; redefine them only if you
want to reshape spacing or rounding.

### 4.3 Defining your own tokens

You may declare any custom property you like. Namespace yours to avoid clashing
with future substrate tokens — e.g. `--mytheme-hero-gradient`. The substrate
doesn't validate tokens it doesn't recognize; they're just CSS.

### 4.4 Color tokens specifically

The contrast verifier resolves contrast-bearing tokens to concrete colors. It
understands a **bounded** color syntax:

- Hex: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`
- `rgb(r, g, b)` / `rgba(r, g, b, a)` (channels 0–255 or `%`; alpha 0–1)
- `color-mix(in srgb, <colorA> <pct>%, <colorB> <pct>%)`
- `var(--other-token)` (resolved through your theme's token map)
- the keywords `white`, `black`, `transparent`

**Pitfall — named colors.** CSS named colors other than `white`/`black`/
`transparent` (e.g. `rebeccapurple`, `steelblue`) are **not** resolvable by the
verifier. If you use one in a *contrast-bearing* token, the theme fails closed
(`theme.contrast.failed: … could not resolve a color value`). Use hex or `rgb()`
for those tokens.

**Pitfall — translucent values.** A translucent token (`rgba(...,0.5)` or
`color-mix(..., transparent)`) is flattened over its paired surface before the
ratio is computed (surfaces are flattened over white). A semi-transparent text
or border color can therefore fail contrast even though the opaque color would
pass. Prefer opaque values for the contrast-bearing tokens (text, status,
border-focus).

### 4.5 Token resolution and inheritance

The substrate walks your chain root→leaf and concatenates each `tokens.css`, so
the **last** (leaf) declaration of a token wins. To debug "why is this token
resolving to X?": the served, resolved stylesheet is available at
`/theme/active.css?id=<your-theme-id>` — fetch it and read the final declaration
of the token in question.

---

## Chapter 5 — Effect classes

### 5.1 What they are, and when to use them

An effect class is a reusable *composition* — a particular gradient, glow, or
glass treatment — that surfaces apply by class name. Use an effect class (rather
than baking the styling into a component or rebuilding it from tokens at every
call site) when you want a treatment that themes can override. Effect classes
live in `effects.css`.

### 5.2 The substrate's baseline effect classes

`aurora-default`'s `effects.css` defines a sixteen-class baseline. By group:

- **Background:** `effect-surface-elevation-1/2/3`, `effect-frosted-glass`,
  `effect-gradient-subtle`, `effect-gradient-accent`
- **Border:** `effect-border-subtle`, `effect-border-glow`, `effect-border-double`
- **Interactive:** `effect-hover-lift`, `effect-hover-glow`, `effect-focus-ring`,
  `effect-active-press`
- **Decorative:** `effect-decorative-noise`, `effect-decorative-pattern-subtle`,
  `effect-aurora-glow`

In `aurora-default` the decorative classes are intentionally trivial (the class
exists but renders nothing) so the sober baseline stays sober. `aurora-dark` and
`aurora-stack-classic` override several with gradients and glows — read their
`effects.css` for worked examples.

### 5.3 Overriding a substrate effect class

Redefine the same class name in your `effects.css`. Compose from tokens so your
override stays portable:

```css
/* a more pronounced frosted glass than aurora-default's */
.effect-frosted-glass {
  background: linear-gradient(
    135deg,
    color-mix(in srgb, var(--color-surface-secondary) 95%, var(--color-accent-primary) 5%),
    color-mix(in srgb, var(--color-surface-secondary) 90%, var(--color-accent-primary) 10%)
  );
  backdrop-filter: blur(12px) saturate(1.2);
  -webkit-backdrop-filter: blur(12px) saturate(1.2);
}
```

### 5.4 Defining your own effect classes

Add new classes for your theme's surfaces. Namespace them so they don't read as
substrate classes — e.g. `.effect-mytheme-starfield`. The substrate doesn't
validate theme-specific effect classes; surfaces that don't apply them simply
don't get the effect.

### 5.5 The required effect classes

Every theme must define these, directly or by inheritance:

- `effect-focus-ring` — a visible focus indicator (you may change its color and
  offset, but it must stay visible — it's an accessibility requirement).
- `effect-surface-elevation-1`, `-2`, `-3` — the structural elevations the
  substrate uses for cards, panels, and drawers.

A theme missing one (and not inheriting it) is rejected:
`theme.effects.required.missing: <class>`. Most themes inherit all four from
`aurora-default` and never redeclare them.

### 5.6 Composing effect classes

Surfaces stack effect classes (`class="effect-surface-elevation-1
effect-frosted-glass effect-hover-lift"`). The substrate enforces no composition
rules — normal CSS cascade applies. Design classes that compose cleanly: use
`box-shadow` for elevation (not `background`) so it layers with a
`background`-based glass effect rather than clobbering it.

---

## Chapter 6 — Extension points

### 6.1 What they are, and when to use them

An extension point is a theme-specific surface treatment a theme *declares it
provides* — something beyond the substrate's required and optional tokens and
its baseline effect classes. A theme that wants to introduce a treatment the
substrate's vocabulary doesn't cover (a cosmic hero background, an emphasis
glow, a custom nav indicator) declares it as an extension point; a surface that
wants that treatment opts in *only when the active theme provides it*, and falls
back to a default otherwise.

The difference from effect classes (Chapter 5) is who decides:

- **Effect classes** are universal-with-overrides. The substrate's surfaces
  *always* apply `effect-focus-ring`, `effect-surface-elevation-1`, and the
  rest; a theme overrides the *implementation* but the class names are part of
  every theme's contract.
- **Extension points** are opt-in-by-theme. A surface checks
  `themeProvidesExtension()` and applies the treatment only when the active
  theme declares it. Themes that don't declare it produce the default; themes
  that do produce the enhancement. Both are valid — it's the theme author's
  choice.

Use an extension point when the treatment is *yours* (not something every theme
should provide) and a surface should light it up conditionally. Use an effect
class when the treatment is universal and every theme should supply it.

### 6.2 Declaring an extension point

Two steps: name it in the manifest, define it in `extensions.css`.

Declare the names in `providedExtensionPoints` and point `files.extensions` at
your stylesheet:

```json
{
  "extends": "aurora-default",
  "providedExtensionPoints": ["hero-treatment-cosmic", "accent-emphasis-glow"],
  "files": {
    "tokens": "tokens.css",
    "extensions": "extensions.css"
  }
}
```

Define each one in `extensions.css` as a class named `.extension-<name>`:

```css
.extension-hero-treatment-cosmic {
  background: linear-gradient(180deg, #000018, var(--color-surface-primary));
  position: relative;
}

.extension-accent-emphasis-glow {
  box-shadow: 0 0 32px color-mix(in srgb, var(--color-accent-primary) 35%, transparent);
}
```

Compose from the inherited token contract (`var(--color-*)`, `color-mix(...)`)
so your extension points stay portable across the palette.

The substrate validates the pairing at theme-load: every name in
`providedExtensionPoints` must have a matching `.extension-<name>` rule, in this
theme's `extensions.css` **or an inherited one** (extension points are additive
across the `extends` chain — see §6.2's note below). A declared name with no
definition fails with `theme.extensions.declared.undefined: <name>`; a name
listed twice fails with `theme.extensions.declared.duplicate: <name>`. A theme
that fails validation is listed but not activatable, same as any other
validation failure (Chapter 8).

**Inheritance is additive.** Unlike tokens and effect classes (where a child
overrides its parent), a child theme's extension points *add to* its parent's —
the parent's remain available unless the child redefines the same
`.extension-<name>` rule. The substrate serves the chain-concatenated result at
`/theme/active-extensions.css` (root first, so a child's redefinition wins),
loaded alongside `active.css` and `active-effects.css`.

### 6.3 How surfaces opt into extension points

A surface asks the runtime whether the active theme provides a point, and
applies the class only when it does:

```js
if (AuroraThemeRuntime.themeProvidesExtension('hero-treatment-cosmic')) {
  heroEl.classList.add('extension-hero-treatment-cosmic');
}
// else: the surface keeps its default treatment
```

`AuroraThemeRuntime.themeProvidesExtension(name)` returns a boolean. A few
properties worth knowing as a theme author:

- **It's a global, not an import.** The admin UI is a no-build, no-bundler
  vanilla-JS app, so the runtime is the `AuroraThemeRuntime` global rather than
  an `import { … } from '@aurora-locus/theme-runtime'` module. (If you read the
  substrate design notes, that import form is illustrative pseudocode; the
  shipped surface is the global.)
- **It's synchronous and cached.** The runtime loads the active theme's
  *effective* extension points (yours plus everything you inherit) once at
  theme-load, from the `/theme/active-extension-points` endpoint, and refreshes
  the cache when the operator switches themes. Surfaces call it freely; there's
  no per-call request.
- **It's fail-soft.** If the list hasn't loaded yet, or the name isn't provided,
  the call returns `false` — the surface renders its default. An extension-point
  lookup never throws and never blocks a render.

This is the whole contract: declare it, define it, and a surface that knows
about it lights it up when your theme is active.

### 6.4 Naming conventions

Extension-point names are a shared namespace across every installed theme, so
name them so two themes' points don't collide and a surface author can tell what
a point *is* from its name:

- **Be semantic, not decorative-adjective.** `hero-treatment-cosmic`, not
  `cosmic`; `accent-emphasis-glow`, not `glow`. The name should read as *what
  surface it treats* + *how*, so a surface author searching for "hero" finds it.
- **Lead with the surface or concept.** Group related points by prefix
  (`hero-…`, `nav-…`, `accent-…`) so they sort together in the theme picker and
  read as a family.
- **Don't shadow substrate vocabulary.** Don't name an extension point after a
  required token or an `effect-*` class; the `.extension-` prefix keeps the CSS
  selectors distinct, but a *name* that mirrors a substrate concept misleads
  surface authors about what it is.
- **Lowercase, hyphen-separated** (matching token and effect-class style):
  `[a-z0-9-]`.

### 6.5 When to recommend surface authors integrate

An extension point only does something if a surface opts into it, so a treatment
you want to *guarantee* shows up isn't an extension point — it's a token or an
effect-class override (which surfaces already apply). Extension points are the
right tool when you and a surface author coordinate: you provide a treatment,
they add the conditional `themeProvidesExtension()` check.

Make that coordination easy:

- **Document your extension points** — what each one treats, what it expects of
  the element it's applied to (a positioned container? a full-bleed section?),
  and the class name. The `examples/extension-point-author/` reference theme's
  README is the model: one short paragraph per point plus the consuming snippet.
- **Operators can already see them.** The theme picker (Configuration → Themes)
  surfaces each theme's `providedExtensionPoints`, so an operator evaluating
  your theme sees what it adds beyond the baseline without reading its CSS.
- **Treat the names as stable.** Once a surface author integrates against
  `hero-treatment-cosmic`, renaming it silently drops the treatment (the
  surface's check just starts returning `false`). Rename like any public API:
  keep the old name defined during a transition, or coordinate the change.

---

## Chapter 7 — Accessibility: the contrast contract

### 7.1 WCAG 2.2 in 200 words

Contrast ratio measures the luminance difference between a foreground and its
background, from 1:1 (identical) to 21:1 (black on white). WCAG 2.2 sets floors:
**4.5:1** for normal body text (AA), **3:1** for large text and for UI
components like focus rings and status indicators (AA), and **7:1** for the
strictest normal-text tier (AAA). Aurora-Locus verifies a fixed set of token
pairs against these floors and **rejects** any theme that misses one. You are
free to exceed them — `aurora-default` aims for AAA where it can.

### 7.2 The contrast pairs the substrate verifies

| Foreground | Background | Required |
|---|---|---|
| `--color-text-primary` | `--color-surface-primary` | 7.0:1 |
| `--color-text-primary` | `--color-surface-secondary` | 7.0:1 |
| `--color-text-primary` | `--color-surface-tertiary` | 4.5:1 |
| `--color-text-secondary` | `--color-surface-primary` | 4.5:1 |
| `--color-text-secondary` | `--color-surface-secondary` | 4.5:1 |
| `--color-text-tertiary` | `--color-surface-primary` | 3.0:1 |
| `--color-text-inverted` | `--color-accent-primary` | 4.5:1 |
| `--color-border-focus` | `--color-surface-primary` | 3.0:1 |
| `--color-border-focus` | `--color-surface-secondary` | 3.0:1 |
| `--color-status-success` | `--color-surface-primary` | 3.0:1 |
| `--color-status-warning` | `--color-surface-primary` | 3.0:1 |
| `--color-status-danger` | `--color-surface-primary` | 3.0:1 |
| `--color-status-info` | `--color-surface-primary` | 3.0:1 |

### 7.3 Tools for checking contrast during development

- Your browser devtools' color picker reports contrast ratios against a chosen
  background.
- Online checkers (e.g. the WebAIM contrast checker) take two hex values.
- The substrate's own report: a failed theme's card on the Themes page lists its
  `theme.contrast.failed` entries with the measured ratio and the required
  threshold, so you can see exactly which pair and by how much.

### 7.4 Common contrast failures

- **`text-inverted` on `accent-primary`.** Button text on the accent fill. A
  mid-tone accent often fails 4.5:1 with *either* a light or a dark inverted
  text. On a light theme, darken the accent so white inverted text passes; on a
  dark theme, a lighter accent lets dark inverted text pass.
- **`text-tertiary` on `surface-primary`.** Tertiary text is the dimmest; it
  only needs 3:1 but a too-light grey misses it.
- **Translucent borders/text.** See §4.4 — flattening can drop a value below the
  floor.
- **Accent used as text.** Accent colors are tuned for fills, not for small text
  on a surface; don't reuse `--color-accent-primary` as a text token.

### 7.5 Beyond contrast

The substrate enforces contrast only. The rest of accessibility is yours:

- **Focus visibility** — keep `effect-focus-ring` visible (don't override it to
  `outline: none`).
- **Motion** — respect `prefers-reduced-motion`; keep your `--motion-duration-*`
  values modest.
- **Font size** — `--font-size-base` below ~14px harms legibility.

### 7.6 What's guaranteed, and what isn't

The substrate guarantees the 13 contrast pairs meet their thresholds for every
*installed* theme. It does **not** check non-token contrast (e.g. text you place
on a custom gradient), motion, or font sizing — those are the theme author's
responsibility.

---

## Chapter 8 — Inheritance: extending bundled themes

### 8.1 Basics

`extends` names your parent. The substrate resolves a token by taking the
leaf-most declaration across the chain; a token you don't declare falls through
to the parent, and so on up to `aurora-default`. The same applies to effect
classes (`effects.css` follows the identical inheritance model).

### 8.2 Extending `aurora-default`

The common case: start from the sober dark root and override a handful of
tokens. Your `tokens.css` lists *only* your overrides; everything else is
inherited. This is the `minimal-accent-override` example.

### 8.3 Extending a bundled reference theme

Set `extends` to `aurora-light`, `aurora-dark`, or `aurora-stack-classic` to
inherit more than structure — a light base, a richer dark base, or the classic
identity (fonts, gradient wordmark, lit-up effects). You then tweak from there.

### 8.4 Chaining inheritance

A theme may extend another operator-custom theme, which extends a bundled theme,
which extends `aurora-default`. The chain must terminate at `aurora-default` and
be at most **4 levels deep** — a deeper chain is rejected
(`theme.chain.too-deep`), and a chain that doesn't reach `aurora-default` is
rejected (`theme.chain.orphan`). A cycle (A extends B extends A) is rejected
(`theme.chain.cycle`).

### 8.5 What you can't inherit-and-remove

You can override a required token but you can't *un-declare* it — the contract is
"every required token has a value somewhere in the chain," and since the root
defines all 28, they're always present. There's no way to make a required token
absent; that's the point.

### 8.6 Migration patterns

When your parent theme changes a token, your theme inherits the new value
automatically (you didn't override it). When the parent adds a token, you inherit
it for free. You only need to act when you've *overridden* a token whose parent
meaning changed — see Chapter 10.

---

## Chapter 9 — Validation and troubleshooting

### 9.1 The validation steps, in order

The substrate validates each theme at startup, in this order. The first failing
step (and any later ones it can still evaluate) is reported:

1. **Manifest parse** — `manifest.json` exists and is valid JSON. (A parse
   failure skips the theme at enumeration; it never reaches validation.)
2. **Schema** — supported `schemaVersion` (`"1.0"`).
3. **Directory match** — `themeId` equals the directory name.
4. **Substrate version** — `substrateVersion` ≤ the running substrate.
5. **Chain** — `extends` resolves, the chain reaches `aurora-default`, no cycle,
   depth ≤ 4.
6. **Token file** — the declared `tokens.css` exists and is readable.
7. **Required tokens** — all 28 are defined in the chain.
8. **Required effect classes** — `effect-focus-ring` and
   `effect-surface-elevation-1/2/3` are defined in the chain.
9. **Contrast** — the 13 pairs meet their thresholds.

(Step 10, extension-point declaration validation, arrives with extension points
in 0.9.1.)

### 9.2 Each failure entry, decoded

| Reason | Meaning | Fix |
|---|---|---|
| `theme.invalid.manifest: unsupported schemaVersion '…'` | Step 2 | Use `"schemaVersion": "1.0"`. |
| `theme.id.directory.mismatch: directory '…' != themeId '…'` | Step 3 | Make `themeId` equal the folder name. |
| `theme.substrate.version.future: targets … > runtime …` | Step 4 | Lower `substrateVersion`, or upgrade Aurora-Locus. |
| `theme.extends.missing: '…'` | Step 5 | The named parent isn't installed. Install it or fix the id. |
| `theme.chain.orphan: … roots at '…', not 'aurora-default'` | Step 5 | Your chain must terminate at `aurora-default`. |
| `theme.chain.cycle: … re-enters '…'` | Step 5 | Break the inheritance loop. |
| `theme.chain.too-deep: … exceeds max inheritance depth 4` | Step 5 | Flatten the chain to ≤ 4 levels. |
| `theme.tokens.file.missing: …` | Step 6 | `files.tokens` points at a missing file. |
| `theme.tokens.required.missing: <token>` | Step 7 | Define the token (or inherit it — check `extends`). |
| `theme.effects.required.missing: <class>` | Step 8 | Define the effect class, or inherit it. |
| `theme.contrast.failed: <fg> on <bg> = X:1 (need Y:1)` | Step 9 | Adjust the colors — see §7.4. |
| `theme.contrast.failed: <fg> on <bg> — could not resolve a color value` | Step 9 | A contrast token uses an unsupported color syntax (e.g. a named color) — use hex/`rgb()`. |

### 9.3 Reading validation results

Configuration → Themes shows a card per installed theme with a **Valid** or
**Failed** badge. A failed theme exposes a *View validation errors* affordance
that lists its `theme.*` reasons. The same reasons are in the startup log.

### 9.4 Common author mistakes

- Forgetting `extends` on a non-root theme → `theme.chain.orphan`.
- `themeId` not matching the folder name → `theme.id.directory.mismatch`.
- A `color-mix()` that resolves to a non-AA value at runtime → it passes a
  by-eye check but fails the verifier; read the measured ratio in the report.
- A typo in a CSS variable name → the token is treated as *missing* (the
  misspelled one isn't the required name) → `theme.tokens.required.missing`.
- A named color in a contrast token → `could not resolve a color value`.

### 9.5 Debugging contrast failures specifically

The pairs easiest to miss are `text-inverted` on `accent-primary` (4.5:1) and
the three `surface-tertiary`/`text-tertiary` pairs. Fix without changing your
aesthetic by nudging *lightness* while keeping hue: darken the accent on a light
theme, lighten it on a dark theme, until the measured ratio in the Themes-page
report clears the threshold. The report tells you the exact ratio and target, so
iterate against the number, not by eye.

---

## Chapter 10 — Forward-compatibility and theme maintenance

This chapter is about what happens to your theme when Aurora-Locus updates. The
short version: **you inherit everything you don't override**, so most updates
reach your theme on their own and you do nothing. This chapter explains when
that holds, the one case where it doesn't, and how the substrate tells you.

Two version numbers matter, and they're separate:

- The **Aurora-Locus version** (e.g. `0.9.0`) — the application release.
- The **substrate version** — the theming contract's own version, declared by
  your theme's `substrateVersion` and checked at load. It is **`1.0`** today and
  moves independently of the application version. Everything below is about the
  substrate version.

### 10.1 The substrate-version compatibility table

`substrateVersion` is a **floor**: it's the *minimum* substrate your theme
needs. The running substrate loads any theme whose declared `substrateVersion`
is **≤** its own, and rejects one that targets a *newer* substrate (it might use
tokens or classes this version doesn't have) — see step 4 in §9.1 and the
`theme.substrate.version.future` entry in §9.2.

| Substrate version | Shipped in | Contract |
|---|---|---|
| `1.0` | Aurora-Locus 0.9 | 28 required tokens (§4.1); 4 required effect classes (§5.5); the optional `aurora-default` token set (§4.2); the effect-class library (§5.2); extension points (§6); lifecycle-hook *declaration* (§10.6). |

There is one substrate version so far. As the contract grows, rows are added
here; a theme keeps working as long as its declared floor stays ≤ the running
substrate. Declare the *lowest* version that has everything you use — that
maximises the range of Aurora-Locus releases your theme runs on.

### 10.2 Minor releases — what can and can't change

A minor (non-breaking) substrate release only ever **adds** to the contract. It
can:

- add new **optional** tokens to `aurora-default`,
- add new effect classes,
- add new extension points or other opt-in capabilities.

It will **not** rename or remove an existing token, effect class, or extension
point, and it will not make a previously-optional thing newly required in a way
your theme can't already satisfy. Because additions land in the inheritance
root, your theme inherits them automatically (§10.4–10.6). **You do nothing.**
You only revisit your theme in a minor release if you *want* to adopt something
new — never because something broke.

This is not hypothetical: extension points (§6) and lifecycle-hook declaration
(§10.6) were both *added* to the substrate after the first themes shipped, both
under substrate version `1.0`, and no existing theme needed a change.

### 10.3 Breaking releases — what changes, and how it's signalled

A breaking substrate release is one that **renames or removes** a contract item,
or makes a new item required that inheritance can't supply. Those are the only
changes that can invalidate a theme that was valid before. A breaking release
**bumps the substrate version** (e.g. `1.0` → `2.0`).

The substrate signals the mismatch the same way in both directions:

- If your theme targets a **newer** substrate than is running (you upgraded the
  theme but not Aurora-Locus, or moved it to an older deployment), it's rejected
  with `theme.substrate.version.future` and is skipped at load.
- If a release **renamed or removed** something your theme *overrode*, the
  override now points at a name the contract no longer has. You'll see it as the
  matching validation failure — most often `theme.tokens.required.missing` or
  `theme.effects.required.missing` (§9.2) — on the Configuration → Themes page
  and in the startup log.

The fix for a breaking change is always: read the failing `theme.*` reasons
(§9.2), update the renamed/removed references, and raise your `substrateVersion`
to the new floor once your theme uses the new contract.

### 10.4 New tokens in a release

When a release adds a token, it adds it to `aurora-default` (the root every
chain terminates at). Your theme `extends` that chain, so the new token
**resolves through inheritance with no change on your part** — whether it's
optional or part of the required set, it's already defined upstream and your
theme satisfies the contract automatically.

You only act if you want the new token to look *different* in your theme: define
it in your own `tokens.css`, exactly as you'd override any inherited token
(§4.5). If you leave it alone, you get the substrate's value. There is no case
where a newly-added token forces you to edit a theme just to keep it valid.

### 10.5 Effect-class additions and modifications

Effect classes follow the same inherit-by-default rule as tokens. A release that
adds an effect class adds it upstream; your theme inherits it and can use it
immediately, or override it in your `effects.css` if you want a different
treatment (§5.3).

One nuance specific to effect classes: because they're *CSS rules* rather than
single values, a substrate release may **refine** an existing class (e.g. adjust
what `effect-frosted-glass` renders) without renaming it. If you have **not**
overridden that class, you inherit the refinement automatically. If you **have**
overridden it, your version keeps winning — the cascade still resolves to your
definition — so a refinement upstream never silently changes a class you've
taken ownership of. The trade-off is the migration note in §8.6: when you
override an effect class, you also opt out of future improvements to it, so
re-check your overrides against the substrate's current version periodically.

### 10.6 Extension-point and lifecycle-hook compatibility

**Extension points** (§6) are additive and opt-in by construction. A surface
checks whether the active theme provides a point and falls back to its default
when it doesn't, so:

- A theme that declares no extension points is unaffected by any number of new
  ones.
- A theme that declares an extension point keeps providing it across releases;
  the substrate doesn't remove a point your theme defines.
- New extension points added by a release are simply available for you to adopt
  (declare them in your manifest and define `.extension-<name>` in
  `extensions.css`); ignoring them costs nothing.

**Lifecycle hooks** are forward-compatible by design. A theme may *declare*
lifecycle hooks — `install`, `activate`, and `deactivate` — as
`--theme-<phase>-hook` custom properties in `extensions.css`. In this version
the substrate **recognises and lists declared hooks but does not execute them**:
script execution opens a sandboxing surface that ships only once it has been
fully specified and security-reviewed. Declared hooks appear on the
Configuration → Themes page as *declared, not run in this version*, and a line
is logged at startup for each.

The forward-compatibility guarantee is this: **you can write and declare hook
scripts now, and they begin running automatically when hook execution lands** —
no theme change required at that point. Until then, a declared hook is inert and
never affects whether your theme loads. If your theme needs setup *today* that a
hook would eventually perform (loading a font, say), do it with CSS-only means —
an `@import url()` at the top of your `tokens.css` works and needs no hook.

### 10.7 Keeping your theme current — a short checklist

- Pin the **lowest** `substrateVersion` that covers what you use (§10.1).
- After an Aurora-Locus upgrade, glance at Configuration → Themes: a **Valid**
  badge means you're done. A **Failed** badge means a breaking change touched
  something you overrode — read the `theme.*` reasons (§9.2) and §10.3.
- Re-check any **overridden** tokens or effect classes against the current
  substrate now and then; overriding opts you out of upstream improvements
  (§10.5, §8.6).
- You never need to act on *additions* to keep a theme valid — only on
  *renames/removals*, which are breaking and version-bumped (§10.3).

> **A living section.** §10.1's table and §10.3's guidance grow a concrete entry
> each time a real substrate change ships — that version-by-version migration
> log accrues over the project's life. The rules above hold regardless of how
> many rows the table eventually has.

---

## Example themes

Complete, copy-able reference themes live under
[`examples/`](examples/). Each is a working theme you can drop into
`<data-dir>/themes/` (alongside the bundled parents it extends) and activate:

- [`examples/minimal-accent-override/`](examples/minimal-accent-override/) — the
  minimum-viable theme: a handful of accent tokens over `aurora-default`.
- [`examples/full-token-redefinition/`](examples/full-token-redefinition/) — a
  theme that declares all 28 required tokens from scratch.
- [`examples/effect-class-customization/`](examples/effect-class-customization/)
  — a theme overriding several effect classes.
- [`examples/high-contrast/`](examples/high-contrast/) — a theme tuned for AAA
  across the board; useful as an accessibility reference.

(The `extension-point-author` example ships with Chapter 6 in 0.9.1.)
