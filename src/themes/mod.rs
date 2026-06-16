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

/// The inheritance root every chain must terminate at (§11.9).
pub const ROOT_THEME_ID: &str = "aurora-default";

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
    pub metadata: serde_json::Value,
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
                    metadata: m.metadata.clone(),
                }
            })
            .collect()
    }

    /// (total, valid) counts — for the startup summary log.
    pub fn summary(&self) -> (usize, usize) {
        let valid = self.records.iter().filter(|r| r.valid).count();
        (self.records.len(), valid)
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
        Some(css)
    }
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

/// Validation contract (§11.10.1), steps 1–9 + chain. Returns the list of
/// failures (empty = valid). Step 10 (extension-point declaration validation)
/// is deferred to 0.9.1 with the extension-point system (recon §4.4).
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

    errors
}

/// Walk the `extends` chain from `start`, asserting it terminates at the
/// root (`aurora-default`, whose `extends` is `None`), has no cycle, and is
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
        // a theme that extends a non-root with no path to aurora-default
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
        assert!(reg.resolve_token_css("aurora-default").is_none());
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
        // The four shipped themes must enumerate and pass the full validation
        // contract (steps 1–9) against the real on-disk files — this is the
        // accessibility/structure guard for the bundled palettes.
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("static/admin/themes");
        let reg = ThemeRegistry::build(&bundled, Path::new("/nonexistent/operator"));
        let (total, valid) = reg.summary();
        assert_eq!(total, 4, "four bundled themes enumerate");
        let invalid: Vec<_> = reg
            .list()
            .into_iter()
            .filter(|m| !m.valid)
            .map(|m| format!("{}: {:?}", m.theme_id, m.validation_errors))
            .collect();
        assert_eq!(valid, 4, "all bundled themes valid; failures: {invalid:?}");

        // The classic theme's gradient wordmark + resolved chain are present.
        let classic = reg
            .resolve_effect_css("aurora-stack-classic")
            .expect("classic effects resolve");
        assert!(classic.contains(".heading-aurora"));
        assert!(classic.contains(".effect-focus-ring"), "inherits required focus-ring");
    }

    #[test]
    fn version_gt_basics() {
        assert!(version_gt("1.1", "1.0"));
        assert!(version_gt("2.0", "1.9"));
        assert!(!version_gt("1.0", "1.0"));
        assert!(!version_gt("1.0", "1.1"));
    }
}
