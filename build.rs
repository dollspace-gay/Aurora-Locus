//! Build-time grep linter for the centralized chain-write invariant
//! (v0.7 arc 1 / v07_audit_coherence.md §6.3 deliverable 2).
//!
//! The audit chain's tamper-evidence depends on every
//! `INSERT INTO audit_chain_entry` going through the centralized
//! helpers in `src/admin/audit_chain.rs`. The helpers acquire the
//! chain-serialization primitive (Postgres advisory lock or SQLite
//! `AppendChainGuard` + DB-write-lock) before reading the chain
//! head and writing the next entry; a raw INSERT bypassing the
//! helpers would race the head read, branching the chain.
//!
//! This script walks `src/` and fails the build if any file
//! outside the allowlist contains the literal string
//! `INSERT INTO audit_chain_entry`. The grep is exhaustive (every
//! line of every `.rs` file under `src/`); macro-generated SQL is
//! not currently used for audit-chain writes, but if a future
//! refactor introduces it the linter would miss it — flag at that
//! point so a stronger mechanism can replace this one.
//!
//! Failure mode: build fails with the offending file + line and a
//! pointer to the centralized helpers. The diagnostic mentions
//! `audit_chain::insert_chain_entry` so the operator-author has a
//! direct path to the fix.

use std::fs;
use std::path::Path;

const PATTERN: &str = "INSERT INTO audit_chain_entry";

/// Files allowed to contain raw `INSERT INTO audit_chain_entry`. Only
/// the centralized helper module qualifies. Paths are matched against
/// the file's path as a suffix so workspace-vs-package invocation
/// differences don't cause false negatives.
const ALLOWED_FILES: &[&str] = &["src/admin/audit_chain.rs"];

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");

    let src_dir = Path::new("src");
    if !src_dir.exists() {
        // Workspace-level or out-of-tree invocation with no source to
        // check; let cargo proceed.
        return;
    }

    let mut violations = Vec::new();
    visit_dir(src_dir, &mut violations);

    if !violations.is_empty() {
        for (path, line_num, line) in &violations {
            println!("cargo:warning={path}:{line_num}: {line}");
        }
        panic!(
            "build error: {} file(s) contain raw `INSERT INTO audit_chain_entry` outside \
             the centralized helper module ({}). Use `audit_chain::insert_chain_entry` \
             (caller-managed transaction) or `audit_chain::insert_chain_entry_pool` \
             (self-managed transaction) instead. See \
             docs/internal/design/v07_audit_coherence.md §6.3 for the structural-enforcement \
             rationale and src/admin/audit_chain.rs's module-level doc for the chain-write \
             invariant.",
            violations.len(),
            ALLOWED_FILES.join(", "),
        );
    }
}

fn visit_dir(dir: &Path, violations: &mut Vec<(String, usize, String)>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, violations);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            scan_file(&path, violations);
        }
    }
}

fn scan_file(path: &Path, violations: &mut Vec<(String, usize, String)>) {
    let path_str = path.to_string_lossy().replace('\\', "/");
    if ALLOWED_FILES.iter().any(|allowed| path_str.ends_with(allowed)) {
        return;
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for (idx, line) in content.lines().enumerate() {
        if line.contains(PATTERN) {
            violations.push((path_str.clone(), idx + 1, line.trim().to_string()));
        }
    }
}
