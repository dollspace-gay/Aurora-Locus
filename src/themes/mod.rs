//! Theming substrate (§11) — theme enumeration, manifest validation,
//! inheritance resolution, and resolved-token serving.
//!
//! Themes live in two roots (recon §4 / SD1): bundled themes ship read-only
//! under `static/admin/themes/`; operator themes under `<data-dir>/themes/`.
//! [`ThemeRegistry::build`] enumerates both at startup, validates each
//! against the §11.10 contract, and (via [`ThemeRegistry::resolve_token_css`])
//! serves the active theme's inheritance-resolved token CSS to the admin UI.
//!
//! Validation here implements steps 1–7 + the chain check (5) of §11.10.1.
//! Step 8 (effect-class completeness) lands with the effect library
//! (B-effects); step 9 (contrast verification) with B-contrast-verifier
//! (#214); step 10 (extension-point declarations) with the extension-point
//! system (0.9.1). Each is appended to `validate` by its arc.

pub mod contrast;
pub mod manifest;

use manifest::{ThemeManifest, SUBSTRATE_VERSION};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The 28 required design tokens (§11.5.1). A valid theme defines every one,
/// directly or by inheritance.
pub const REQUIRED_TOKENS: &[&str] = &[
    "--color-surface-primary",
    "--color-surface-secondary",
    "--color-surface-tertiary",
    "--color-surface-overlay",
    "--color-text-primary",
    "--color-text-secondary",
    "--color-text-tertiary",
    "--color-text-inverted",
    "--color-accent-primary",
    "--color-accent-primary-hover",
    "--color-accent-primary-active",
    "--color-accent-secondary",
    "--color-status-success",
    "--color-status-warning",
    "--color-status-danger",
    "--color-status-info",
    "--color-border-primary",
    "--color-border-secondary",
    "--color-border-focus",
    "--font-family-sans",
    "--font-family-mono",
    "--font-family-display",
    "--font-size-base",
    "--space-unit",
    "--motion-duration-fast",
    "--motion-duration-medium",
    "--motion-duration-slow",
    "--motion-easing-standard",
];

/// Effect classes every theme must provide directly or by inheritance
/// (§11.6.5): a visible focus indicator (accessibility) and the three
/// structural surface elevations the substrate uses for cards/panels/drawers.
/// A theme missing these renders inaccessible focus or collapsed surfaces.
pub const REQUIRED_EFFECT_CLASSES: &[&str] = &[
    "effect-focus-ring",
    "effect-surface-elevation-1",
    "effect-surface-elevation-2",
    "effect-surface-elevation-3",
];

/// The three lifecycle-hook custom properties a theme may declare in its
/// `extensions.css` (§11.8), mapping each lifecycle phase to the CSS custom
/// property that registers its hook script. §11.8.1 spells out the property for
/// `onInstall` (`--theme-install-hook`); the `onActivate`/`onDeactivate`
/// property names extend the same convention (a translation per the design's
/// stated three-hook set, recorded here since §11.8.1 only names install).
///
/// v0.9 treats these as **declaration-aware no-ops** (§11.8.4): the substrate
/// detects and surfaces declared hooks and logs that execution is off, but does
/// not fetch or run any hook script. Hook *execution* opens a code-execution
/// surface the design defers until the sandboxing model is specified and
/// security-reviewed; only then does this become the wiring point.
pub const LIFECYCLE_HOOK_PROPERTIES: &[(&str, &str)] = &[
    ("install", "--theme-install-hook"),
    ("activate", "--theme-activate-hook"),
    ("deactivate", "--theme-deactivate-hook"),
];

/// The inheritance root every chain must terminate at (§11.9). The former
/// `aurora-default` + `aurora-dark` pair was merged into a single `dark` root.
pub const ROOT_THEME_ID: &str = "dark";

/// Maximum inheritance depth (§11.4) — a chain of more than this many
/// `extends` hops is rejected.
const MAX_CHAIN_DEPTH: usize = 4;

/// Which root a theme was discovered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSource {
    Bundled,
    Operator,
}

/// A theme discovered on disk (manifest parsed, not yet validated).
#[derive(Debug, Clone)]
struct DiscoveredTheme {
    manifest: ThemeManifest,
    dir: PathBuf,
    source: ThemeSource,
}

/// A theme after validation — what the registry holds.
struct ThemeRecord {
    discovered: DiscoveredTheme,
    valid: bool,
    errors: Vec<String>,
}

/// A lifecycle hook a theme declares in its `extensions.css` (§11.8). Surfaced
/// to operators so a theme's declared-but-dormant hooks are visible, and logged
/// at startup as execution-off. Carries no behavior in v0.9 — see
/// [`LIFECYCLE_HOOK_PROPERTIES`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredHook {
    /// The lifecycle phase — `install`, `activate`, or `deactivate`.
    pub phase: String,
    /// The script reference the theme registered for that phase (the
    /// custom-property value, surrounding quotes stripped). Recorded verbatim;
    /// never fetched or executed in v0.9.
    pub script: String,
}

/// One valid theme's declared lifecycle hooks (§11.8) — the startup
/// execution-off report shape. Only themes that declare at least one hook
/// appear in [`ThemeRegistry::lifecycle_hook_report`].
#[derive(Debug, Clone)]
pub struct ThemeLifecycleHooks {
    pub theme_id: String,
    pub hooks: Vec<DeclaredHook>,
}

/// Operator-facing metadata for one installed theme — the `listInstalled`
/// wire shape (§11.10.2).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeMetadata {
    pub theme_id: String,
    pub theme_name: String,
    pub theme_version: Option<String>,
    pub theme_author: Option<String>,
    pub theme_description: Option<String>,
    pub extends: Option<String>,
    pub source: ThemeSource,
    pub valid: bool,
    pub validation_errors: Vec<String>,
    /// Extension points this theme declares (§11.7 / #285). The theme picker
    /// surfaces these so operators see what a theme adds beyond the baseline
    /// (§11.7.3 discovery). Own declarations only — the effective (inherited)
    /// set for the *active* theme is served by `/theme/active-extension-points`.
    pub provided_extension_points: Vec<String>,
    /// Lifecycle hooks this theme declares in its `extensions.css` (§11.8). v0.9
    /// surfaces them but does not execute them (declaration-aware no-op,
    /// §11.8.4), so the picker can show a theme carries dormant hooks. Empty for
    /// the common case (no theme declares any today).
    pub declared_lifecycle_hooks: Vec<DeclaredHook>,
    pub metadata: serde_json::Value,
}

/// Per-theme WCAG 2.2 contrast certification result (#321). See
/// [`ThemeRegistry::wcag_report`] for the AA/AAA criteria.
#[derive(Debug, Clone)]
pub struct ThemeWcag {
    pub theme_id: String,
    /// Meets the substrate's §11.10.3 gate (≥ WCAG 2.2 AA, stricter on body text).
    pub aa: bool,
    /// Every text-on-surface pair additionally clears 7:1 (WCAG 2.2 AAA, normal text).
    pub aaa: bool,
    /// The weakest contrast pair's ratio.
    pub min_ratio: f64,
    /// The weakest pair, as `"<fg-token> on <bg-token>"`.
    pub min_pair: String,
}

/// The installed-theme registry, built once at startup.
pub struct ThemeRegistry {
    records: Vec<ThemeRecord>,
}

impl ThemeRegistry {
    /// Enumerate + validate themes from the two roots. Bundled is scanned
    /// first; an operator theme reusing a bundled id is skipped (bundled is
    /// authoritative). Missing roots are fine (no themes there).
    pub fn build(bundled_root: &Path, operator_root: &Path) -> Self {
        let mut discovered = Vec::new();
        discovered.extend(enumerate_root(bundled_root, ThemeSource::Bundled));
        discovered.extend(enumerate_root(operator_root, ThemeSource::Operator));

        let mut by_id: HashMap<String, DiscoveredTheme> = HashMap::new();
        for d in discovered {
            let id = d.manifest.theme_id.clone();
            if by_id.contains_key(&id) {
                tracing::warn!(
                    theme_id = %id,
                    "duplicate theme id across roots; keeping the bundled/first and skipping the later"
                );
                continue;
            }
            by_id.insert(id, d);
        }

        let mut records: Vec<ThemeRecord> = by_id
            .values()
            .map(|d| {
                let errors = validate(d, &by_id);
                if !errors.is_empty() {
                    for e in &errors {
                        tracing::warn!(theme_id = %d.manifest.theme_id, reason = %e, "theme failed validation");
                    }
                }
                ThemeRecord {
                    discovered: d.clone(),
                    valid: errors.is_empty(),
                    errors,
                }
            })
            .collect();
        records.sort_by(|a, b| {
            a.discovered
                .manifest
                .theme_id
                .cmp(&b.discovered.manifest.theme_id)
        });
        ThemeRegistry { records }
    }

    /// Operator-facing list of installed themes (valid + invalid), for the
    /// Configuration → Themes page.
    pub fn list(&self) -> Vec<ThemeMetadata> {
        self.records
            .iter()
            .map(|r| {
                let m = &r.discovered.manifest;
                ThemeMetadata {
                    theme_id: m.theme_id.clone(),
                    theme_name: m.theme_name.clone(),
                    theme_version: m.theme_version.clone(),
                    theme_author: m.theme_author.clone(),
                    theme_description: m.theme_description.clone(),
                    extends: m.extends.clone(),
                    source: r.discovered.source,
                    valid: r.valid,
                    validation_errors: r.errors.clone(),
                    provided_extension_points: m.provided_extension_points.clone(),
                    declared_lifecycle_hooks: collect_declared_lifecycle_hooks(&r.discovered),
                    metadata: m.metadata.clone(),
                }
            })
            .collect()
    }

    /// Declared lifecycle hooks per *valid* theme that declares any (§11.8), for
    /// the startup execution-off log. v0.9 detects and reports hooks but runs
    /// none (§11.8.4); this is the operational record that a theme carries a
    /// dormant hook. Themes declaring no hooks (the common case) are omitted.
    pub fn lifecycle_hook_report(&self) -> Vec<ThemeLifecycleHooks> {
        self.records
            .iter()
            .filter(|r| r.valid)
            .filter_map(|r| {
                let hooks = collect_declared_lifecycle_hooks(&r.discovered);
                if hooks.is_empty() {
                    None
                } else {
                    Some(ThemeLifecycleHooks {
                        theme_id: r.discovered.manifest.theme_id.clone(),
                        hooks,
                    })
                }
            })
            .collect()
    }

    /// (total, valid) counts — for the startup summary log.
    pub fn summary(&self) -> (usize, usize) {
        let valid = self.records.iter().filter(|r| r.valid).count();
        (self.records.len(), valid)
    }

    /// Per-theme WCAG 2.2 contrast certification over the resolved token maps
    /// (#321), for every *valid* theme. `aa` = every §11.10.3 contrast pair
    /// meets the substrate gate's threshold (≥ WCAG 2.2 AA throughout, and
    /// stricter — 7:1 — on primary body text); this is exactly the step-9 gate,
    /// so it holds for any theme the registry kept. `aaa` additionally requires
    /// every text-on-surface pair to clear 7:1 (WCAG 2.2 1.4.6, normal text)
    /// while UI-component pairs (focus ring, status dots) hold at their AA 3:1
    /// floor — WCAG defines no AAA tier for non-text contrast. `min_ratio` is
    /// the weakest pair, for the report line.
    pub fn wcag_report(&self) -> Vec<ThemeWcag> {
        // The resolver walks `extends` by id, so rebuild the id→theme map.
        let by_id: HashMap<String, DiscoveredTheme> = self
            .records
            .iter()
            .map(|r| (r.discovered.manifest.theme_id.clone(), r.discovered.clone()))
            .collect();
        self.records
            .iter()
            .filter(|r| r.valid)
            .map(|r| {
                let id = &r.discovered.manifest.theme_id;
                let values = collect_declared_token_values(id, &by_id);
                let pairs = contrast::report(&values);
                let is_text = |p: &contrast::PairContrast| p.fg.starts_with("--color-text-");
                let aa = pairs.iter().all(|p| p.ratio + 1e-3 >= p.required);
                let aaa = pairs.iter().all(|p| {
                    let need = if is_text(p) { 7.0 } else { p.required };
                    p.ratio + 1e-3 >= need
                });
                let weakest = pairs
                    .iter()
                    .min_by(|a, b| a.ratio.partial_cmp(&b.ratio).unwrap_or(std::cmp::Ordering::Equal));
                let (min_ratio, min_pair) = match weakest {
                    Some(p) => (p.ratio, format!("{} on {}", p.fg, p.bg)),
                    None => (0.0, String::new()),
                };
                ThemeWcag { theme_id: id.clone(), aa, aaa, min_ratio, min_pair }
            })
            .collect()
    }

    /// Resolve the active theme's token CSS: walk its `extends` chain
    /// root→leaf and concatenate each theme's `tokens.css` so the leaf's
    /// later declarations win. Falls back to the root theme if `id` is
    /// missing or invalid. Returns `None` only if even the root is absent
    /// (no bundled themes installed yet — the admin UI then keeps using its
    /// static `tokens.css`).
    pub fn resolve_token_css(&self, id: &str) -> Option<String> {
        self.resolve_chain_css(id, |files| Some(files.tokens.as_str()))
    }

    /// Resolve the active theme's effect-class CSS (§11.6): same chain walk as
    /// [`resolve_token_css`], over each theme's optional `effects.css` so a
    /// leaf's redefinition of an effect class overrides its ancestors'. Themes
    /// without an `effects.css` contribute nothing (they inherit). Returns
    /// `None` only if even the root is absent.
    pub fn resolve_effect_css(&self, id: &str) -> Option<String> {
        self.resolve_chain_css(id, |files| files.effects.as_deref())
    }

    /// Resolve the active theme's extension-point CSS (§11.7): same chain walk
    /// as [`resolve_effect_css`], over each theme's optional `extensions.css`.
    /// Extension points are **additive** (§11.7) — the chain concatenates
    /// root→leaf so an inherited `.extension-*` rule survives unless a leaf
    /// redefines it. Returns `None` only if even the root is absent.
    pub fn resolve_extension_css(&self, id: &str) -> Option<String> {
        self.resolve_chain_css(id, |files| files.extensions.as_deref())
    }

    /// The active theme's **effective** extension points — its own
    /// `providedExtensionPoints` plus every ancestor's, deduped, root→leaf
    /// order (§11.7 additive semantics). This is what the frontend runtime's
    /// `themeProvidesExtension(name)` resolves membership against. Returns an
    /// empty list when the theme (or root fallback) is absent or declares none.
    pub fn resolve_extension_points(&self, id: &str) -> Vec<String> {
        let find_valid = |want: &str| {
            self.records
                .iter()
                .find(|r| r.valid && r.discovered.manifest.theme_id == want)
        };
        let Some(target) = find_valid(id).or_else(|| find_valid(ROOT_THEME_ID)) else {
            return Vec::new();
        };
        let by_id: HashMap<&str, &ThemeRecord> = self
            .records
            .iter()
            .map(|r| (r.discovered.manifest.theme_id.as_str(), r))
            .collect();

        // Walk leaf→root collecting declarations, then emit root→leaf deduped.
        let mut chain: Vec<&ThemeRecord> = Vec::new();
        let mut cursor: Option<&ThemeRecord> = Some(target);
        let mut guard = 0;
        while let Some(rec) = cursor {
            guard += 1;
            if guard > MAX_CHAIN_DEPTH + 2 {
                break;
            }
            chain.push(rec);
            cursor = match &rec.discovered.manifest.extends {
                Some(parent) => by_id.get(parent.as_str()).copied(),
                None => None,
            };
        }
        chain.reverse();

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for rec in chain {
            for name in &rec.discovered.manifest.provided_extension_points {
                if seen.insert(name.clone()) {
                    out.push(name.clone());
                }
            }
        }
        out
    }

    /// Shared chain walk for resolved CSS: locate the requested valid theme
    /// (or the root), build its `extends` chain root→leaf, and concatenate the
    /// file selected by `file_of` from each theme so later (leaf) source wins.
    fn resolve_chain_css<F>(&self, id: &str, file_of: F) -> Option<String>
    where
        F: Fn(&manifest::ThemeFiles) -> Option<&str>,
    {
        let find_valid = |want: &str| {
            self.records
                .iter()
                .find(|r| r.valid && r.discovered.manifest.theme_id == want)
        };
        let target = find_valid(id).or_else(|| find_valid(ROOT_THEME_ID))?;

        let by_id: HashMap<&str, &ThemeRecord> = self
            .records
            .iter()
            .map(|r| (r.discovered.manifest.theme_id.as_str(), r))
            .collect();

        // Build leaf→root, then reverse so the root is emitted first.
        let mut chain: Vec<&ThemeRecord> = Vec::new();
        let mut cursor: Option<&ThemeRecord> = Some(target);
        let mut guard = 0;
        while let Some(rec) = cursor {
            guard += 1;
            if guard > MAX_CHAIN_DEPTH + 2 {
                break; // chain already validated; this is a belt-and-braces stop
            }
            chain.push(rec);
            cursor = match &rec.discovered.manifest.extends {
                Some(parent) => by_id.get(parent.as_str()).copied(),
                None => None,
            };
        }
        chain.reverse();

        let mut css = String::new();
        for rec in chain {
            if let Some(file) = file_of(&rec.discovered.manifest.files) {
                let path = rec.discovered.dir.join(file);
                if let Ok(content) = std::fs::read_to_string(&path) {
                    css.push_str(&content);
                    css.push('\n');
                }
            }
        }
        Some(hoist_imports(&css))
    }
}

/// Hoist `@import` statements to the top of a concatenated stylesheet. A
/// theme's `tokens.css` may carry an `@import` (aurora-classic loads
/// Google Fonts), but the chain concatenates leaf-last, so a leaf's `@import`
/// lands mid-file where browsers ignore it (CSS requires `@import` before all
/// other rules). This moves every line-leading `@import` to the front in source
/// order. Line-based (the bundled themes keep each `@import` on its own line).
fn hoist_imports(css: &str) -> String {
    if !css.contains("@import") {
        return css.to_string();
    }
    let mut imports = Vec::new();
    let mut rest = String::new();
    for line in css.lines() {
        if line.trim_start().starts_with("@import") {
            imports.push(line.trim());
        } else {
            rest.push_str(line);
            rest.push('\n');
        }
    }
    if imports.is_empty() {
        return css.to_string();
    }
    let mut out = String::new();
    for imp in imports {
        out.push_str(imp);
        out.push('\n');
    }
    out.push_str(&rest);
    out
}

/// Scan a root for `<root>/<id>/manifest.json`. A missing root yields an
/// empty list; subdirectories without a manifest are ignored; a manifest
/// that fails to parse is warned-and-skipped.
fn enumerate_root(root: &Path, source: ThemeSource) -> Vec<DiscoveredTheme> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        match ThemeManifest::parse_file(&manifest_path) {
            Ok(manifest) => out.push(DiscoveredTheme {
                manifest,
                dir,
                source,
            }),
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "theme manifest parse failed; skipping")
            }
        }
    }
    out
}

/// Validation contract (§11.10.1), steps 1–10 + chain. Returns the list of
/// failures (empty = valid). Step 10 (extension-point declaration validation)
/// landed with the 0.9.1 extension-point system (#285).
fn validate(theme: &DiscoveredTheme, by_id: &HashMap<String, DiscoveredTheme>) -> Vec<String> {
    let mut errors = Vec::new();
    let m = &theme.manifest;

    // Step 2 — schema basics beyond serde: supported schemaVersion.
    if m.schema_version != "1.0" {
        errors.push(format!(
            "theme.invalid.manifest: unsupported schemaVersion '{}'",
            m.schema_version
        ));
    }

    // Step 3 — themeId matches the directory name.
    let dir_name = theme.dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if dir_name != m.theme_id {
        errors.push(format!(
            "theme.id.directory.mismatch: directory '{}' != themeId '{}'",
            dir_name, m.theme_id
        ));
    }

    // Step 4 — substrateVersion not newer than this runtime's.
    if version_gt(&m.substrate_version, SUBSTRATE_VERSION) {
        errors.push(format!(
            "theme.substrate.version.future: targets {} > runtime {}",
            m.substrate_version, SUBSTRATE_VERSION
        ));
    }

    // Step 5 — extends chain: parent exists, terminates at the root, no
    // cycle, depth <= MAX_CHAIN_DEPTH.
    let chain_ok = match validate_chain(&m.theme_id, by_id) {
        Ok(()) => true,
        Err(e) => {
            errors.push(e);
            false
        }
    };

    // Step 6 — the declared token file exists + is readable.
    let tokens_path = theme.dir.join(&m.files.tokens);
    if std::fs::read_to_string(&tokens_path).is_err() {
        errors.push(format!(
            "theme.tokens.file.missing: {}",
            tokens_path.display()
        ));
    }

    // Step 7 — required-token completeness (own + inherited). Only meaningful
    // when the chain is sane (otherwise inheritance can't be resolved).
    let mut required_complete = false;
    if chain_ok {
        let declared = collect_declared_tokens(&m.theme_id, by_id);
        let before = errors.len();
        for tok in REQUIRED_TOKENS {
            if !declared.contains(*tok) {
                errors.push(format!("theme.tokens.required.missing: {tok}"));
            }
        }
        required_complete = errors.len() == before;
    }

    // Step 8 — required effect-class completeness (§11.6.5). The substrate
    // uses these classes for focus indication and surface elevation; a theme
    // missing them (own or inherited) produces inaccessible focus or collapsed
    // surfaces. Fail-closed; gated on a sane chain so inheritance resolves.
    if chain_ok {
        let declared = collect_declared_effect_classes(&m.theme_id, by_id);
        for class in REQUIRED_EFFECT_CLASSES {
            if !declared.contains(*class) {
                errors.push(format!("theme.effects.required.missing: {class}"));
            }
        }
    }

    // Step 9 — WCAG 2.2 AA contrast (§11.10.3). Resolve the contrast-requiring
    // token pairs to concrete colors through the chain and verify each meets
    // its threshold (fail-closed). Skipped unless the chain resolves and every
    // required token is present, since the resolved map would otherwise be
    // incomplete and produce noise on top of the step-7 failures.
    if chain_ok && required_complete {
        let values = collect_declared_token_values(&m.theme_id, by_id);
        errors.extend(contrast::verify(&values));
    }

    // Step 10 — extension-point declaration validation (§11.7, #285). Every
    // name in `providedExtensionPoints` must be defined as `.extension-<name>`
    // in the theme's own or inherited `extensions.css` (extension points are
    // additive across the chain); a declared-but-undefined point fails. A
    // duplicate declaration in the list fails. Gated on a sane chain so the
    // inherited `extensions.css` files resolve.
    if chain_ok {
        let mut seen = HashSet::new();
        for name in &m.provided_extension_points {
            if !seen.insert(name.as_str()) {
                errors.push(format!("theme.extensions.declared.duplicate: {name}"));
            }
        }
        if !m.provided_extension_points.is_empty() {
            let defined = collect_declared_extension_points(&m.theme_id, by_id);
            for name in &m.provided_extension_points {
                if !defined.contains(&format!("extension-{name}")) {
                    errors.push(format!("theme.extensions.declared.undefined: {name}"));
                }
            }
        }
    }

    errors
}

/// Walk the `extends` chain from `start`, asserting it terminates at the
/// root (`dark`, whose `extends` is `None`), has no cycle, and is
/// within the depth bound.
fn validate_chain(start: &str, by_id: &HashMap<String, DiscoveredTheme>) -> Result<(), String> {
    let mut current = start.to_string();
    let mut visited = vec![current.clone()];
    for _ in 0..=MAX_CHAIN_DEPTH {
        let theme = by_id
            .get(&current)
            .ok_or_else(|| format!("theme.extends.missing: '{current}'"))?;
        match &theme.manifest.extends {
            None => {
                return if current == ROOT_THEME_ID {
                    Ok(())
                } else {
                    Err(format!(
                        "theme.chain.orphan: chain from '{start}' roots at '{current}', not '{ROOT_THEME_ID}'"
                    ))
                };
            }
            Some(parent) => {
                if visited.iter().any(|v| v == parent) {
                    return Err(format!("theme.chain.cycle: '{start}' re-enters '{parent}'"));
                }
                visited.push(parent.clone());
                current = parent.clone();
            }
        }
    }
    Err(format!(
        "theme.chain.too-deep: '{start}' exceeds max inheritance depth {MAX_CHAIN_DEPTH}"
    ))
}

/// Collect every custom-property name declared in a theme's chain
/// (root→leaf), for the required-token completeness check. Detects
/// `--name:` declarations; not a full CSS parse.
fn collect_declared_tokens(start: &str, by_id: &HashMap<String, DiscoveredTheme>) -> HashSet<String> {
    let re = regex::Regex::new(r"(--[A-Za-z0-9_-]+)\s*:").expect("static token-decl regex compiles");
    let mut tokens = HashSet::new();
    let mut current = start.to_string();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > MAX_CHAIN_DEPTH + 2 {
            break;
        }
        let theme = match by_id.get(&current) {
            Some(t) => t,
            None => break,
        };
        let path = theme.dir.join(&theme.manifest.files.tokens);
        if let Ok(css) = std::fs::read_to_string(&path) {
            for cap in re.captures_iter(&css) {
                tokens.insert(cap[1].to_string());
            }
        }
        match &theme.manifest.extends {
            Some(parent) => current = parent.clone(),
            None => break,
        }
    }
    tokens
}

/// Collect every effect-class name (`.effect-*` selector) declared across a
/// theme's chain (root→leaf), for the required-effect-class completeness check
/// (§11.6.5). Themes without an `effects.css` contribute nothing (they inherit
/// from the parent). Matches class selectors; not a full CSS parse.
fn collect_declared_effect_classes(
    start: &str,
    by_id: &HashMap<String, DiscoveredTheme>,
) -> HashSet<String> {
    let re = regex::Regex::new(r"\.(effect-[A-Za-z0-9_-]+)")
        .expect("static effect-class regex compiles");
    let mut classes = HashSet::new();
    let mut current = start.to_string();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > MAX_CHAIN_DEPTH + 2 {
            break;
        }
        let theme = match by_id.get(&current) {
            Some(t) => t,
            None => break,
        };
        if let Some(effects_file) = &theme.manifest.files.effects {
            let path = theme.dir.join(effects_file);
            if let Ok(css) = std::fs::read_to_string(&path) {
                for cap in re.captures_iter(&css) {
                    classes.insert(cap[1].to_string());
                }
            }
        }
        match &theme.manifest.extends {
            Some(parent) => current = parent.clone(),
            None => break,
        }
    }
    classes
}

/// Collect every extension-point class declared in a theme's chain
/// (`.extension-<name>` rules in each `extensions.css`), for step-10
/// declaration validation (§11.7). Mirrors `collect_declared_effect_classes`,
/// reading `files.extensions` instead of `files.effects`. Extension points
/// are additive across the chain (§11.7 / design §11.6's additive note), so a
/// child's declarations satisfy via either its own or an inherited
/// `extensions.css`. Returns the class names (the `extension-` prefix
/// included), so a `providedExtensionPoints` entry `foo` is checked as
/// `extension-foo`.
fn collect_declared_extension_points(
    start: &str,
    by_id: &HashMap<String, DiscoveredTheme>,
) -> HashSet<String> {
    let re = regex::Regex::new(r"\.(extension-[A-Za-z0-9_-]+)")
        .expect("static extension-point regex compiles");
    let mut classes = HashSet::new();
    let mut current = start.to_string();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > MAX_CHAIN_DEPTH + 2 {
            break;
        }
        let theme = match by_id.get(&current) {
            Some(t) => t,
            None => break,
        };
        if let Some(extensions_file) = &theme.manifest.files.extensions {
            let path = theme.dir.join(extensions_file);
            if let Ok(css) = std::fs::read_to_string(&path) {
                for cap in re.captures_iter(&css) {
                    classes.insert(cap[1].to_string());
                }
            }
        }
        match &theme.manifest.extends {
            Some(parent) => current = parent.clone(),
            None => break,
        }
    }
    classes
}

/// Detect the lifecycle hooks a theme declares in its OWN `extensions.css`
/// (§11.8) — the `--theme-<phase>-hook` custom properties of
/// [`LIFECYCLE_HOOK_PROPERTIES`]. Own-file only: hooks carry no execution in
/// v0.9, so the effective (inherited-cascade) resolution that would matter for
/// *running* a hook is deferred with execution; what an operator needs now is
/// what each theme itself declares. Surrounding quotes on the value are
/// stripped. Not a full CSS parse (mirrors the other `collect_*` scanners).
fn collect_declared_lifecycle_hooks(theme: &DiscoveredTheme) -> Vec<DeclaredHook> {
    let Some(extensions_file) = &theme.manifest.files.extensions else {
        return Vec::new();
    };
    let path = theme.dir.join(extensions_file);
    let Ok(css) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut hooks = Vec::new();
    for (phase, prop) in LIFECYCLE_HOOK_PROPERTIES {
        // `--theme-<phase>-hook : "..." ;` — value up to the next `;` or `}`.
        let re = regex::Regex::new(&format!(r#"{}\s*:\s*([^;}}]+)"#, regex::escape(prop)))
            .expect("static lifecycle-hook regex compiles");
        if let Some(cap) = re.captures(&css) {
            let script = cap[1]
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .trim()
                .to_string();
            if !script.is_empty() {
                hooks.push(DeclaredHook {
                    phase: (*phase).to_string(),
                    script,
                });
            }
        }
    }
    hooks
}

/// Collect every custom-property declaration's resolved *value* across a
/// theme's chain (leaf-wins), for contrast verification. Within a single file
/// the last declaration wins (CSS cascade); across the chain the leaf overrides
/// its ancestors. Captures `--name: value` up to the next `;` or `}`; not a
/// full CSS parse.
fn collect_declared_token_values(
    start: &str,
    by_id: &HashMap<String, DiscoveredTheme>,
) -> HashMap<String, String> {
    let re = regex::Regex::new(r"(--[A-Za-z0-9_-]+)\s*:\s*([^;}]+)")
        .expect("static token-decl regex compiles");
    let mut values: HashMap<String, String> = HashMap::new();
    let mut current = start.to_string();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > MAX_CHAIN_DEPTH + 2 {
            break;
        }
        let theme = match by_id.get(&current) {
            Some(t) => t,
            None => break,
        };
        let path = theme.dir.join(&theme.manifest.files.tokens);
        if let Ok(css) = std::fs::read_to_string(&path) {
            // Per-file last-wins, then merge into the accumulator without
            // overwriting (leaf processed first → leaf wins across files).
            let mut file_map: HashMap<String, String> = HashMap::new();
            for cap in re.captures_iter(&css) {
                file_map.insert(cap[1].to_string(), cap[2].trim().to_string());
            }
            for (k, v) in file_map {
                values.entry(k).or_insert(v);
            }
        }
        match &theme.manifest.extends {
            Some(parent) => current = parent.clone(),
            None => break,
        }
    }
    values
}

/// Compare two `major.minor` version strings; `true` iff `a > b`. Unparseable
/// components are treated as 0 (lenient — semver enforcement isn't the job).
fn version_gt(a: &str, b: &str) -> bool {
    fn parts(s: &str) -> (u32, u32) {
        let mut it = s.split('.');
        let major = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        let minor = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        (major, minor)
    }
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a theme dir with a manifest + tokens.css + a complete effects.css
    /// under `root`. The effects.css carries every required effect class so the
    /// fixture passes step 8; tests that need an effect gap override it.
    fn write_theme(root: &Path, id: &str, extends: Option<&str>, tokens: &str) {
        write_theme_full(root, id, extends, tokens, Some(&all_required_effects()));
    }

    /// Like [`write_theme`] but with explicit control over `effects.css`:
    /// `None` omits the file and its manifest entry (the theme inherits effects
    /// from its parent).
    fn write_theme_full(
        root: &Path,
        id: &str,
        extends: Option<&str>,
        tokens: &str,
        effects: Option<&str>,
    ) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let ext = match extends {
            Some(e) => format!("\"extends\": \"{e}\",\n"),
            None => String::new(),
        };
        let files = match effects {
            Some(_) => r#""files":{"tokens":"tokens.css","effects":"effects.css"}"#,
            None => r#""files":{"tokens":"tokens.css"}"#,
        };
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{"schemaVersion":"1.0","themeId":"{id}","themeName":"{id}","substrateVersion":"1.0",{ext}{files}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("tokens.css"), tokens).unwrap();
        if let Some(css) = effects {
            std::fs::write(dir.join("effects.css"), css).unwrap();
        }
    }

    /// An effects.css declaring every required effect class (§11.6.5).
    fn all_required_effects() -> String {
        REQUIRED_EFFECT_CLASSES
            .iter()
            .map(|c| format!(".{c} {{ outline: 0; }}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A contrast-passing palette covering all required tokens: backgrounds
    /// (surfaces + accent-primary, which is the `text-inverted` backdrop) are
    /// white, every color foreground is black (21:1), and non-color tokens get
    /// a placeholder. Keeps validation fixtures valid under step 9.
    fn all_required_css() -> String {
        REQUIRED_TOKENS
            .iter()
            .map(|t| {
                let val = match *t {
                    "--color-surface-primary"
                    | "--color-surface-secondary"
                    | "--color-surface-tertiary"
                    | "--color-accent-primary" => "#ffffff",
                    x if x.starts_with("--color-") => "#000000",
                    _ => "0",
                };
                format!("{t}: {val};")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("aurora-themes-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn root_plus_child_validate_and_resolve() {
        let bundled = tmp("rootchild");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        // child overrides only one token; inherits the rest from root.
        write_theme(&bundled, "child", Some(ROOT_THEME_ID), "--color-accent-primary: #f00;");
        let empty = bundled.join("__no_operator__");

        let reg = ThemeRegistry::build(&bundled, &empty);
        let (total, valid) = reg.summary();
        assert_eq!(total, 2);
        assert_eq!(valid, 2, "child inherits required tokens from root");

        let css = reg.resolve_token_css("child").expect("resolves");
        // root emitted first, child after → child's accent override wins.
        let last = css.rfind("--color-accent-primary").unwrap();
        assert!(css[last..].contains("#f00"));
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn missing_required_token_fails() {
        let bundled = tmp("missingtoken");
        write_theme(&bundled, ROOT_THEME_ID, None, "--color-accent-primary: #000;");
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        let (_, valid) = reg.summary();
        assert_eq!(valid, 0, "root missing 27 required tokens is invalid");
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn orphan_chain_fails() {
        let bundled = tmp("orphan");
        // a theme that extends a non-root with no path to the dark root
        write_theme(&bundled, "lonely", Some("ghost"), &all_required_css());
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        let (_, valid) = reg.summary();
        assert_eq!(valid, 0, "extends a missing parent → invalid");
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn directory_mismatch_fails() {
        let bundled = tmp("mismatch");
        // manifest themeId says 'right' but dir is 'wrong'
        let dir = bundled.join("wrong");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"schemaVersion":"1.0","themeId":"right","themeName":"R","substrateVersion":"1.0","files":{"tokens":"tokens.css"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("tokens.css"), all_required_css()).unwrap();
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert_eq!(reg.summary().1, 0);
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn missing_roots_are_fine() {
        let reg = ThemeRegistry::build(
            Path::new("/nonexistent/bundled"),
            Path::new("/nonexistent/operator"),
        );
        assert_eq!(reg.summary(), (0, 0));
        assert!(reg.resolve_token_css("dark").is_none());
        assert!(reg.list().is_empty());
    }

    #[test]
    fn low_contrast_theme_fails_validation() {
        let bundled = tmp("lowcontrast");
        // Start from the passing palette, then override text/surface to a
        // near-isoluminant grey pair (well below the 7:1 requirement).
        let mut css = all_required_css();
        css.push_str("\n--color-text-primary: #999999;\n--color-surface-primary: #888888;");
        write_theme(&bundled, ROOT_THEME_ID, None, &css);
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert_eq!(
            reg.summary().1,
            0,
            "text/surface below WCAG AA fails step 9 (fail-closed)"
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn contrast_resolves_through_var_inheritance() {
        let bundled = tmp("varinherit");
        // Root carries the passing palette. Child re-points text-primary at a
        // var() that resolves (through the child's own declaration) to a color
        // still meeting contrast against the inherited white surface.
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        write_theme(
            &bundled,
            "child",
            Some(ROOT_THEME_ID),
            "--ink: #111111;\n--color-text-primary: var(--ink);",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert_eq!(reg.summary().1, 2, "var()-routed text resolves and passes");
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn missing_required_effect_class_fails() {
        let bundled = tmp("missingeffect");
        // All tokens present + an effects.css that omits effect-surface-elevation-3.
        let partial_effects = ".effect-focus-ring { outline: 2px solid red; }\n\
             .effect-surface-elevation-1 { box-shadow: 0 1px 2px #000; }\n\
             .effect-surface-elevation-2 { box-shadow: 0 2px 4px #000; }";
        write_theme_full(
            &bundled,
            ROOT_THEME_ID,
            None,
            &all_required_css(),
            Some(partial_effects),
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert_eq!(
            reg.summary().1,
            0,
            "missing effect-surface-elevation-3 fails step 8 (fail-closed)"
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn child_inherits_required_effects_from_parent() {
        let bundled = tmp("inheriteffects");
        // Root carries the full effect library; child has no effects.css and
        // inherits all four required classes through the chain.
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        // Override a token outside the contrast pairs so this test isolates
        // effect inheritance from the (separately-tested) contrast gate.
        write_theme_full(
            &bundled,
            "child",
            Some(ROOT_THEME_ID),
            "--color-accent-secondary: #2563eb;",
            None,
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert_eq!(reg.summary().1, 2, "child inherits required effect classes");

        // resolve_effect_css emits the root's effects for the child too.
        let css = reg.resolve_effect_css("child").expect("resolves");
        assert!(css.contains(".effect-focus-ring"));
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn bundled_themes_all_validate() {
        // The shipped themes must enumerate and pass the full validation
        // contract (steps 1–9) against the real on-disk files — this is the
        // accessibility/structure guard for the bundled palettes. The full
        // v0.9 cohort: dark root + light + aurora-classic + the 7 showcase
        // themes (ember, emerald, glacier, meridian, high-contrast-dark,
        // high-contrast-light, pride).
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("static/admin/themes");
        let reg = ThemeRegistry::build(&bundled, Path::new("/nonexistent/operator"));
        let (total, valid) = reg.summary();
        assert_eq!(total, 10, "bundled themes enumerate");
        let invalid: Vec<_> = reg
            .list()
            .into_iter()
            .filter(|m| !m.valid)
            .map(|m| format!("{}: {:?}", m.theme_id, m.validation_errors))
            .collect();
        assert_eq!(valid, 10, "all bundled themes valid; failures: {invalid:?}");

        // The classic theme's gradient wordmark + resolved chain are present.
        let classic = reg
            .resolve_effect_css("aurora-classic")
            .expect("classic effects resolve");
        assert!(classic.contains(".heading-aurora"));
        assert!(classic.contains(".effect-focus-ring"), "inherits required focus-ring");
    }

    #[test]
    fn wcag_certification_report() {
        // #321 — programmatic WCAG 2.2 certification over the real bundled
        // cohort. Every theme must clear the substrate's contrast gate (AA, and
        // stricter on body text); the two high-contrast themes must additionally
        // clear AAA (every text-on-surface pair ≥ 7:1). Prints the per-theme
        // report; run with `-- --nocapture` to read it.
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("static/admin/themes");
        let reg = ThemeRegistry::build(&bundled, Path::new("/nonexistent/operator"));
        let report = reg.wcag_report();
        assert_eq!(report.len(), 10, "all ten themes certify");

        eprintln!("\n=== WCAG 2.2 certification — bundled themes (#321) ===");
        let mut sorted = report.clone();
        sorted.sort_by(|a, b| a.theme_id.cmp(&b.theme_id));
        for c in &sorted {
            let tier = if c.aaa { "AAA" } else if c.aa { "AA " } else { "FAIL" };
            eprintln!(
                "  {:<20} {}  (weakest {:.2}:1 — {})",
                c.theme_id, tier, c.min_ratio, c.min_pair
            );
        }

        // AA across the whole cohort.
        for c in &report {
            assert!(c.aa, "{} fails WCAG 2.2 AA (weakest {:.2}:1)", c.theme_id, c.min_ratio);
        }
        // AAA for the two high-contrast themes specifically.
        for id in ["high-contrast-dark", "high-contrast-light"] {
            let c = report.iter().find(|c| c.theme_id == id).expect("HC theme certifies");
            assert!(c.aaa, "{id} must clear WCAG 2.2 AAA (weakest {:.2}:1)", c.min_ratio);
        }
    }

    #[test]
    fn classic_tokens_hoist_import_to_top() {
        // aurora-classic's @import is in its (leaf) tokens.css, so the
        // root→leaf concatenation puts it mid-file; hoisting must lift it back
        // to the top where browsers honor it.
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("static/admin/themes");
        let reg = ThemeRegistry::build(&bundled, Path::new("/nonexistent/operator"));
        let css = reg
            .resolve_token_css("aurora-classic")
            .expect("classic resolves");
        // The Google Fonts @import (the only `@import url(...)`; the header
        // comment mentions the word in prose) must lead the stylesheet and
        // precede the first :root.
        assert!(
            css.trim_start().starts_with("@import url"),
            "@import hoisted to the top"
        );
        let import_pos = css.find("@import url").expect("has the fonts import");
        let root_pos = css.find(":root").expect("has a :root block");
        assert!(import_pos < root_pos, "@import precedes the first :root rule");
    }

    #[test]
    fn aurora_classic_replaces_stack_classic_in_registry() {
        // #406 rename: the bundled classic theme is `aurora-classic`; the old
        // `stack-classic` id must be gone (no stale dir, no stale manifest id),
        // and aurora-classic must still resolve its signature gradient wordmark.
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("static/admin/themes");
        let reg = ThemeRegistry::build(&bundled, Path::new("/nonexistent/operator"));
        let ids: Vec<String> = reg.list().into_iter().map(|m| m.theme_id).collect();
        assert!(
            ids.iter().any(|id| id == "aurora-classic"),
            "aurora-classic present; got {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id == "stack-classic"),
            "stack-classic fully renamed; got {ids:?}"
        );
        let classic = reg
            .resolve_effect_css("aurora-classic")
            .expect("aurora-classic effects resolve");
        assert!(classic.contains(".heading-aurora"), "keeps the gradient wordmark");
    }

    #[test]
    fn sidebar_tokens_derive_from_theme_surfaces() {
        // #406: the sidebar token family is plumbed through each theme's own
        // surface/text tokens in base tokens.css :root (so the rail coheres with
        // the active palette), and the former hardcoded [data-theme="dark"]
        // sidebar override is removed (auto-derivation, no bespoke). This guards
        // the coherence contract structurally; actual contrast is guaranteed by
        // wcag_certification_report (text-primary on surface-secondary/-tertiary).
        let tokens = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("static/admin/styles/tokens.css"),
        )
        .expect("base tokens.css readable");
        for (token, anchor) in [
            ("--sidebar-bg", "var(--color-surface-secondary)"),
            ("--sidebar-text", "var(--color-text-primary)"),
            ("--sidebar-active", "var(--color-surface-tertiary)"),
            ("--sidebar-hover", "var(--color-surface-tertiary)"),
        ] {
            assert!(
                tokens.contains(&format!("{token}: {anchor};")),
                "{token} must derive from {anchor}"
            );
        }
        // No hardcoded hex sidebar value survives (the slate default + the dark
        // override are both gone — the family is purely derived now).
        assert!(
            !tokens.contains("--sidebar-bg: #"),
            "no hardcoded --sidebar-bg hex remains"
        );
    }

    #[test]
    fn version_gt_basics() {
        assert!(version_gt("1.1", "1.0"));
        assert!(version_gt("2.0", "1.9"));
        assert!(!version_gt("1.0", "1.0"));
        assert!(!version_gt("1.0", "1.1"));
    }

    // ---------- §11.7 step-10 extension-point declaration validation (#285) ----------

    /// Write a complete valid theme that also declares `providedExtensionPoints`
    /// plus a `files.extensions` `extensions.css` with the given body. Tokens
    /// and effects are the complete contrast-passing fixtures, so only step 10
    /// is exercised.
    fn write_theme_with_extensions(
        root: &Path,
        id: &str,
        extends: Option<&str>,
        provided: &[&str],
        extensions_css: &str,
    ) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let ext = match extends {
            Some(e) => format!("\"extends\":\"{e}\","),
            None => String::new(),
        };
        let provided_json = provided
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{"schemaVersion":"1.0","themeId":"{id}","themeName":"{id}","substrateVersion":"1.0",{ext}"providedExtensionPoints":[{provided_json}],"files":{{"tokens":"tokens.css","effects":"effects.css","extensions":"extensions.css"}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("tokens.css"), all_required_css()).unwrap();
        std::fs::write(dir.join("effects.css"), all_required_effects()).unwrap();
        std::fs::write(dir.join("extensions.css"), extensions_css).unwrap();
    }

    /// Validation errors for one theme id in a freshly-built registry.
    fn errors_for(reg: &ThemeRegistry, id: &str) -> Vec<String> {
        reg.list()
            .into_iter()
            .find(|t| t.theme_id == id)
            .map(|t| t.validation_errors)
            .unwrap_or_default()
    }

    #[test]
    fn extension_point_declared_and_defined_validates() {
        let bundled = tmp("extok");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        write_theme_with_extensions(
            &bundled,
            "exttheme",
            Some(ROOT_THEME_ID),
            &["hero-treatment-cosmic", "aurora-glow-accent"],
            ".extension-hero-treatment-cosmic { opacity: 1; }\n.extension-aurora-glow-accent { opacity: 1; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert!(
            errors_for(&reg, "exttheme").is_empty(),
            "declared + defined extension points validate: {:?}",
            errors_for(&reg, "exttheme")
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn extension_point_declared_but_undefined_fails() {
        let bundled = tmp("extundef");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        // declares two; defines only one.
        write_theme_with_extensions(
            &bundled,
            "exttheme",
            Some(ROOT_THEME_ID),
            &["defined-one", "missing-two"],
            ".extension-defined-one { opacity: 1; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        let errs = errors_for(&reg, "exttheme");
        assert!(
            errs.iter().any(|e| e == "theme.extensions.declared.undefined: missing-two"),
            "undefined extension point must fail: {errs:?}"
        );
        assert!(
            !errs.iter().any(|e| e.contains("defined-one")),
            "the defined one must not fail: {errs:?}"
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn extension_point_duplicate_declaration_fails() {
        let bundled = tmp("extdup");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        write_theme_with_extensions(
            &bundled,
            "exttheme",
            Some(ROOT_THEME_ID),
            &["dup", "dup"],
            ".extension-dup { opacity: 1; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        let errs = errors_for(&reg, "exttheme");
        assert!(
            errs.iter().any(|e| e == "theme.extensions.declared.duplicate: dup"),
            "duplicate declaration must fail: {errs:?}"
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    // ---------- §11.8 lifecycle-hook declaration-aware no-op ----------

    /// Metadata for one theme id in a freshly-built registry.
    fn meta_for(reg: &ThemeRegistry, id: &str) -> ThemeMetadata {
        reg.list().into_iter().find(|t| t.theme_id == id).expect("theme present")
    }

    #[test]
    fn lifecycle_hooks_declared_are_detected_and_surfaced() {
        let bundled = tmp("hookdetect");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        // A theme declaring two of the three hooks in extensions.css. Quotes
        // (double here) are stripped; the third hook is absent.
        write_theme_with_extensions(
            &bundled,
            "hooktheme",
            Some(ROOT_THEME_ID),
            &[],
            ":root {\n  --theme-install-hook: \"/themes/hooktheme/install.js\";\n  --theme-activate-hook: \"/themes/hooktheme/activate.js\";\n}",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));

        // Surfaced on the listInstalled metadata.
        let hooks = meta_for(&reg, "hooktheme").declared_lifecycle_hooks;
        assert_eq!(hooks.len(), 2, "two declared hooks detected: {hooks:?}");
        assert!(hooks.iter().any(|h| h.phase == "install"
            && h.script == "/themes/hooktheme/install.js"));
        assert!(hooks.iter().any(|h| h.phase == "activate"
            && h.script == "/themes/hooktheme/activate.js"));
        assert!(!hooks.iter().any(|h| h.phase == "deactivate"), "no deactivate declared");

        // Reported for the startup execution-off log.
        let report = reg.lifecycle_hook_report();
        let entry = report.iter().find(|t| t.theme_id == "hooktheme").expect("reported");
        assert_eq!(entry.hooks.len(), 2);

        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn lifecycle_hooks_absent_surface_nothing() {
        let bundled = tmp("hooknone");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        // An extensions.css with no hook custom properties.
        write_theme_with_extensions(
            &bundled,
            "plaintheme",
            Some(ROOT_THEME_ID),
            &["feature-x"],
            ".extension-feature-x { opacity: 1; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));

        assert!(
            meta_for(&reg, "plaintheme").declared_lifecycle_hooks.is_empty(),
            "no hooks declared → empty"
        );
        assert!(
            !reg.lifecycle_hook_report().iter().any(|t| t.theme_id == "plaintheme"),
            "themes declaring no hooks are omitted from the report"
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn lifecycle_hook_declaration_does_not_invalidate_theme() {
        // Declaring a hook is valid (§11.8: "themes can declare them"); the
        // no-op layer must not turn a dormant hook into a validation failure.
        let bundled = tmp("hookvalid");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        write_theme_with_extensions(
            &bundled,
            "hooktheme",
            Some(ROOT_THEME_ID),
            &[],
            ":root { --theme-deactivate-hook: \"/themes/hooktheme/cleanup.js\"; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert!(
            errors_for(&reg, "hooktheme").is_empty(),
            "a declared hook must not invalidate the theme: {:?}",
            errors_for(&reg, "hooktheme")
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn extension_point_satisfied_by_inherited_definition() {
        let bundled = tmp("extinherit");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        // Parent defines `.extension-foo`; child declares `foo` but its own
        // extensions.css doesn't define it — additive inheritance satisfies it.
        write_theme_with_extensions(
            &bundled,
            "parentext",
            Some(ROOT_THEME_ID),
            &[],
            ".extension-foo { opacity: 1; }",
        );
        write_theme_with_extensions(
            &bundled,
            "childext",
            Some("parentext"),
            &["foo"],
            "/* child adds no extension definitions of its own */",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        assert!(
            errors_for(&reg, "childext").is_empty(),
            "inherited extension definition satisfies the child's declaration: {:?}",
            errors_for(&reg, "childext")
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn resolve_extension_css_concatenates_chain_additively() {
        let bundled = tmp("extcss");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        write_theme_with_extensions(
            &bundled,
            "parentext",
            Some(ROOT_THEME_ID),
            &[],
            ".extension-parent-fx { opacity: 0.1; }",
        );
        write_theme_with_extensions(
            &bundled,
            "childext",
            Some("parentext"),
            &[],
            ".extension-child-fx { opacity: 0.9; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        let css = reg.resolve_extension_css("childext").expect("resolves");
        assert!(css.contains(".extension-parent-fx"), "inherited rule present: {css}");
        assert!(css.contains(".extension-child-fx"), "own rule present: {css}");
        // Root emitted first → parent's rule precedes the child's.
        assert!(
            css.find(".extension-parent-fx") < css.find(".extension-child-fx"),
            "additive chain order is root→leaf"
        );
        let _ = std::fs::remove_dir_all(&bundled);
    }

    #[test]
    fn resolve_extension_points_unions_chain_deduped() {
        let bundled = tmp("extpts");
        write_theme(&bundled, ROOT_THEME_ID, None, &all_required_css());
        write_theme_with_extensions(
            &bundled,
            "parentext",
            Some(ROOT_THEME_ID),
            &["bar"],
            ".extension-bar { opacity: 0.1; }",
        );
        write_theme_with_extensions(
            &bundled,
            "childext",
            Some("parentext"),
            &["foo"],
            ".extension-foo { opacity: 0.9; }",
        );
        let reg = ThemeRegistry::build(&bundled, &bundled.join("__none__"));
        let points = reg.resolve_extension_points("childext");
        // Root→leaf order: parent's `bar` before child's `foo`.
        assert_eq!(points, vec!["bar".to_string(), "foo".to_string()]);
        let _ = std::fs::remove_dir_all(&bundled);
    }
}
