//! Subprocess test that executes the README's "Initial Setup" bash
//! block verbatim against a freshly-built `aurora-locus` binary.
//!
//! The test extracts the bash block dynamically from `README.md`,
//! sets test values for the documented placeholders, and runs the
//! resulting script via `bash`. This is the regression backstop for
//! the bootstrap docs: any future change that adds a required env
//! var or placeholder to the README without updating the test will
//! cause the test to fail (the spawned subprocess won't have the new
//! var set, the binary will reject, and bash will exit non-zero).
//!
//! v0.3 scope: SQLite-only. Postgres bootstrap requires a running
//! Postgres and is deferred to a Phase B integration suite.

use std::path::PathBuf;
use std::process::Command;

/// Extract the bash block following the `### Initial Setup` heading
/// in `README.md`. Returns `None` if the heading or block is missing
/// — both cases fail the test loudly so the README structure can't
/// drift away from what the test expects.
fn extract_initial_setup_bash_block(readme: &str) -> Option<String> {
    let heading_idx = readme.find("### Initial Setup")?;
    let after_heading = &readme[heading_idx..];
    let bash_open = "```bash\n";
    let bash_start = after_heading.find(bash_open)? + bash_open.len();
    let after_bash = &after_heading[bash_start..];
    let bash_end = after_bash.find("\n```")?;
    Some(after_bash[..bash_end].to_string())
}

#[test]
fn extract_finds_block_when_present() {
    let readme = "intro\n### Initial Setup\n\nblurb\n\n```bash\necho hi\n```\n";
    let block = extract_initial_setup_bash_block(readme).unwrap();
    assert_eq!(block, "echo hi");
}

#[test]
fn extract_returns_none_when_heading_missing() {
    let readme = "intro\n```bash\necho hi\n```\n";
    assert!(extract_initial_setup_bash_block(readme).is_none());
}

#[test]
fn extract_returns_none_when_block_missing_after_heading() {
    let readme = "### Initial Setup\n\nno block here\n";
    assert!(extract_initial_setup_bash_block(readme).is_none());
}

/// Locate the workspace root by walking up from the test binary's
/// CARGO_MANIFEST_DIR until we find a `README.md`. This makes the
/// test robust to where it's invoked from.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
async fn readme_bootstrap_block_executes_cleanly() {
    let workspace = workspace_root();
    let readme_path = workspace.join("README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|e| panic!("read README.md at {}: {}", readme_path.display(), e));

    let bash_block = extract_initial_setup_bash_block(&readme).unwrap_or_else(|| {
        panic!(
            "could not extract bash block under '### Initial Setup' from README.md \
             — either the heading was renamed or the fenced block was removed; \
             update the test extractor to match the README structure"
        )
    });

    // Cargo guarantees the integration test's bin dependency is
    // built before the test runs, and exposes the binary path via
    // `CARGO_BIN_EXE_<name>`. Using this instead of running
    // `cargo build` inline avoids contending with the outer cargo
    // process for the workspace's build lock — which manifests as
    // intermittent failures when this test runs alongside other
    // integration tests that also touch `target/`.
    let aurora_locus_bin = env!("CARGO_BIN_EXE_aurora-locus");
    let bin_dir = std::path::Path::new(aurora_locus_bin)
        .parent()
        .expect("CARGO_BIN_EXE_aurora-locus must have a parent directory");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let augmented_path = format!("{}:{}", bin_dir.display(), existing_path);

    // Tempdir for the test DB + lockfile + actor store. The bash
    // block itself doesn't reference these paths; AppContext::new
    // picks them up from PDS_DATA_DIRECTORY.
    let tempdir = tempfile::tempdir().expect("tempdir");
    let data_dir = tempdir.path();

    // Documented placeholder substitution: every `${VAR}` referenced
    // by the README's bash block (and every env var the README's
    // preamble names as required) must have a value in the spawned
    // subprocess's environment. Adding a placeholder to the README
    // without adding it here causes this test to fail.
    // Run bash from inside the tempdir so dotenv()'s `.env`-file
    // search starts there (and finds nothing). If the workspace's
    // `.env` were picked up, its `PDS_ACCOUNT_DB_LOCATION=./data/...`
    // would override the test's `PDS_DATA_DIRECTORY` derivation and
    // the test would write to the workspace's data dir instead of
    // the tempdir — silently leaking state across runs.
    let output = Command::new("bash")
        .arg("-c")
        .arg(&bash_block)
        .current_dir(data_dir)
        .env_clear()
        .env("PATH", &augmented_path)
        // Bootstrap-critical env vars (named in the README preamble).
        .env(
            "PDS_JWT_SECRET",
            "test-jwt-secret-bootstrap-readme-test-32",
        )
        .env(
            "PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX",
            "a".repeat(64),
        )
        .env(
            "PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX",
            "b".repeat(64),
        )
        // Operator-supplied placeholder named in the README preamble.
        .env("PDS_OPERATOR_DID", "did:plc:test1234567890abcdef")
        // Tempdir for the test's data — keeps the test hermetic and
        // free of cross-test contamination on CI.
        .env("PDS_DATA_DIRECTORY", data_dir)
        // Pin SQLite for v0.3 — Postgres bootstrap is out of scope.
        .env("PDS_DB_BACKEND", "sqlite")
        .output()
        .expect("spawn bash subprocess");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "bootstrap script exited non-zero ({})\n\n--- stdout ---\n{}\n\n--- stderr ---\n{}",
        output.status,
        stdout,
        stderr,
    );

    // Success markers from the documented bootstrap output. The
    // README claims grant-admin prints "Granted role 'superadmin'
    // to <did>. Audit entry: #N." and the chain-walk prints
    // "<N> entry/entries verified, chain healthy.". Both must
    // appear in the subprocess output for the bootstrap to be
    // considered working.
    assert!(
        stdout.contains("Granted role 'superadmin'")
            && stdout.contains("did:plc:test1234567890abcdef")
            && stdout.contains("Audit entry:"),
        "stdout missing grant-admin success line\n--- stdout ---\n{}",
        stdout,
    );
    assert!(
        stdout.contains("entry verified, chain healthy.")
            || stdout.contains("entries verified, chain healthy."),
        "stdout missing chain-walk healthy line\n--- stdout ---\n{}",
        stdout,
    );
}
