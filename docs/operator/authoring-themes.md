# Authoring Aurora-Locus themes

This guide is for **theme authors** — operators who want to restyle the
Aurora-Locus admin by forking a bundled theme, building a theme from scratch,
or tuning one for accessibility. It assumes you have Aurora-Locus running and
know basic CSS. It does *not* cover substrate internals (see the v0.9 design
doc) or general CSS (see [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS)).

The theming substrate ships in Aurora-Locus 0.9.0. This document covers the
0.9.0 surface. Two chapters are forward-dated and noted where they appear:

- **Chapter 6 (Extension points)** ships with the extension-point runtime in
  **0.9.1**.
- **Chapter 10 (Forward-compatibility)** fleshes out as the 0.9.x cycle
  accumulates substrate changes (**0.9.2+**).

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

> **Ships in Aurora-Locus 0.9.1.** Extension points let a theme light up opt-in
> surfaces beyond the substrate's defaults (declared via
> `providedExtensionPoints` in the manifest and defined in `extensions.css`,
> consumed by surfaces through the extension-point runtime). The runtime and
> this chapter's worked examples land with 0.9.1; the
> `examples/extension-point-author/` reference theme ships then. In 0.9.0,
> `providedExtensionPoints` and `files.extensions` are accepted in the manifest
> but not yet executed.

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

> **Fleshes out in 0.9.2+.** This chapter documents how themes survive
> Aurora-Locus updates. The load-bearing rule already holds: because you
> *inherit* everything you don't override, a substrate that adds tokens or effect
> classes in a minor release flows into your theme automatically — you act only
> when you've overridden something whose parent meaning changed, or when a
> breaking release bumps the substrate version (signalled by
> `theme.substrate.version.future` if your `substrateVersion` is now too low).
> Concrete version-by-version guidance accrues as the 0.9.x cycle produces real
> migration scenarios.

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
