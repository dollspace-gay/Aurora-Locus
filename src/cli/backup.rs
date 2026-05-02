//! Database Backup and Restore CLI Commands
//!
//! Provides command-line tools for backing up and restoring the
//! Aurora-Locus database. Dispatches on backend type:
//!
//! - **SQLite**: file copy (with optional gzip) of the .db files.
//! - **Postgres**: shells out to `pg_dump` (logical backup) and
//!   `psql` (restore). See docs/operator/backup-restore.md for the
//!   operator-facing guide.
//!
//! For Postgres, the restore path includes a pre-flight check that
//! warns if the sequencer leader-election advisory lock is held —
//! restoring while another aurora-locus instance is running against
//! the same database is dangerous.

use crate::{
    config::DatabaseBackend,
    context::AppContext,
    error::{PdsError, PdsResult},
};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Backup database to file. Dispatches on the configured backend
/// (chainlink #95 / Phase 5.2): SQLite uses file copy, Postgres uses
/// pg_dump.
pub async fn backup_database(
    ctx: &AppContext,
    output: &str,
    compress: bool,
    all: bool,
) -> PdsResult<()> {
    match ctx.config.database.backend {
        DatabaseBackend::Sqlite => backup_sqlite(ctx, output, compress, all).await,
        DatabaseBackend::Postgres => backup_postgres(ctx, output, compress).await,
    }
}

/// Restore database from backup. Dispatches on the configured backend.
pub async fn restore_database(
    ctx: &AppContext,
    input: &str,
    skip_confirmation: bool,
) -> PdsResult<()> {
    match ctx.config.database.backend {
        DatabaseBackend::Sqlite => restore_sqlite(ctx, input, skip_confirmation).await,
        DatabaseBackend::Postgres => restore_postgres(ctx, input, skip_confirmation).await,
    }
}

/// SQLite backup: file copy of the .db file(s), optionally gzipped.
async fn backup_sqlite(
    ctx: &AppContext,
    output: &str,
    compress: bool,
    all: bool,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Database Backup (SQLite)");
    println!("════════════════════════════════════════════════════════");

    let databases = if all {
        vec![
            ("Account DB", ctx.config.storage.account_db.clone()),
            ("Sequencer DB", ctx.config.storage.sequencer_db.clone()),
            ("DID Cache DB", ctx.config.storage.did_cache_db.clone()),
        ]
    } else {
        vec![("Account DB", ctx.config.storage.account_db.clone())]
    };

    for (name, db_path) in &databases {
        println!("\n📋 Backing up {}...", name);
        println!("Source: {}", db_path.display());

        // Verify source database exists
        if !db_path.exists() {
            println!("⚠️  Skipping {} - file does not exist", name);
            continue;
        }

        // Determine output path
        let output_path = if all {
            // When backing up all databases, append the database name to the output path
            let db_name = db_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("database.db");
            let output_base = Path::new(output);
            if compress {
                output_base.with_file_name(format!("{}.gz", db_name))
            } else {
                output_base.with_file_name(db_name)
            }
        } else {
            PathBuf::from(output)
        };

        println!("Output: {}", output_path.display());

        // Perform backup
        if compress {
            backup_with_compression(db_path, &output_path)?;
            println!("✓ Compressed backup created");
        } else {
            fs::copy(db_path, &output_path).map_err(|e| {
                crate::error::PdsError::Internal(format!("Failed to copy database: {}", e))
            })?;
            println!("✓ Backup created");
        }

        // Show file sizes
        let original_size = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        let backup_size = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);

        println!(
            "Original: {} bytes, Backup: {} bytes",
            format_size(original_size),
            format_size(backup_size)
        );

        if compress {
            let ratio = (backup_size as f64 / original_size as f64) * 100.0;
            println!("Compression ratio: {:.1}%", ratio);
        }
    }

    println!("\n════════════════════════════════════════════════════════");
    println!("✅ Backup completed successfully");
    println!("════════════════════════════════════════════════════════\n");

    Ok(())
}

/// SQLite restore: file copy from backup to the configured account_db
/// path, optionally gunzipping. Creates a `.backup` snapshot of the
/// existing file before overwriting.
async fn restore_sqlite(
    ctx: &AppContext,
    input: &str,
    skip_confirmation: bool,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Database Restore (SQLite)");
    println!("════════════════════════════════════════════════════════");

    let input_path = Path::new(input);
    let db_path = &ctx.config.storage.account_db;

    println!("Source: {}", input_path.display());
    println!("Target: {}", db_path.display());

    // Verify input file exists
    if !input_path.exists() {
        return Err(crate::error::PdsError::NotFound(format!(
            "Backup file not found: {}",
            input_path.display()
        )));
    }

    // Check if file is compressed
    let is_compressed = input_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "gz")
        .unwrap_or(false);

    if is_compressed {
        println!("Format: Compressed (gzip)");
    } else {
        println!("Format: Uncompressed");
    }

    // Confirmation prompt
    if !skip_confirmation {
        println!("\n⚠️  WARNING: This will overwrite the existing database!");
        println!("Current database: {}", db_path.display());
        print!("\nProceed with restore? [y/N]: ");
        io::stdout().flush().unwrap();

        let mut response = String::new();
        io::stdin().read_line(&mut response).map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to read user input: {}", e))
        })?;

        if !response.trim().eq_ignore_ascii_case("y") {
            println!("Restore cancelled.");
            return Ok(());
        }
    }

    println!("\n📦 Restoring database...");

    // Create backup of existing database before overwriting
    if db_path.exists() {
        let backup_path = db_path.with_extension("db.backup");
        println!("Creating safety backup: {}", backup_path.display());
        fs::copy(db_path, &backup_path).map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to create safety backup: {}", e))
        })?;
    }

    // Perform restore
    if is_compressed {
        restore_with_decompression(input_path, db_path)?;
        println!("✓ Database decompressed and restored");
    } else {
        fs::copy(input_path, db_path).map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to restore database: {}", e))
        })?;
        println!("✓ Database restored");
    }

    println!("\n════════════════════════════════════════════════════════");
    println!("✅ Restore completed successfully");
    println!("════════════════════════════════════════════════════════");
    println!("\n⚠️  Please restart the server to use the restored database.\n");

    Ok(())
}

/// Backup database with gzip compression
fn backup_with_compression(source: &Path, destination: &Path) -> PdsResult<()> {
    let input = File::open(source).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to open source database: {}", e))
    })?;

    let output = File::create(destination).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to create output file: {}", e))
    })?;

    let mut encoder = GzEncoder::new(output, Compression::default());
    let mut reader = io::BufReader::new(input);

    io::copy(&mut reader, &mut encoder).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to compress database: {}", e))
    })?;

    encoder.finish().map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to finalize compression: {}", e))
    })?;

    Ok(())
}

/// Restore database with gzip decompression
fn restore_with_decompression(source: &Path, destination: &Path) -> PdsResult<()> {
    let input = File::open(source).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to open backup file: {}", e))
    })?;

    let output = File::create(destination).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to create database file: {}", e))
    })?;

    let mut decoder = GzDecoder::new(input);
    let mut writer = io::BufWriter::new(output);

    io::copy(&mut decoder, &mut writer).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to decompress database: {}", e))
    })?;

    Ok(())
}

// ===========================================================================
// Postgres backup / restore (chainlink #95 / Phase 5.2).
//
// Logical backup via pg_dump → SQL text (optionally gzipped). Restore
// via psql, with a pre-flight check that warns when the sequencer
// leader-election advisory lock is held — restoring while another
// aurora-locus instance is live against the same DB would race.
//
// Physical backup (pg_basebackup) is intentionally out of scope; the
// operator guide documents it as a manual procedure for deployments
// that need it. See docs/operator/backup-restore.md.
// ===========================================================================

/// Postgres backup: invokes `pg_dump` against the configured PDS_DB_URL
/// and writes SQL output to `output`. Optionally gzip-compresses.
async fn backup_postgres(
    ctx: &AppContext,
    output: &str,
    compress: bool,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Database Backup (Postgres)");
    println!("════════════════════════════════════════════════════════");

    let url = postgres_url(ctx)?;
    let output_path = PathBuf::from(output);

    println!("Source: {} (Postgres)", redact_url(&url));
    println!("Output: {}", output_path.display());
    if compress {
        println!("Format: gzip-compressed SQL");
    } else {
        println!("Format: plain SQL");
    }

    // pg_dump --no-owner --no-acl produces a portable backup that can
    // be restored against a fresh database without the original
    // ownership/grants (operators typically restore into a db they
    // own, so reproducing the upstream owner is noise).
    println!("\n📦 Running pg_dump...");
    let mut dump = Command::new("pg_dump")
        .arg("--no-owner")
        .arg("--no-acl")
        .arg(&url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            PdsError::Internal(format!(
                "Failed to invoke pg_dump (is it installed and on PATH?): {}",
                e
            ))
        })?;

    let dump_stdout = dump.stdout.take().expect("pg_dump stdout piped");
    let output_file = File::create(&output_path).map_err(|e| {
        PdsError::Internal(format!(
            "Failed to create output {}: {}",
            output_path.display(),
            e
        ))
    })?;

    let mut reader = io::BufReader::new(dump_stdout);
    let bytes_written = if compress {
        let mut encoder = GzEncoder::new(output_file, Compression::default());
        let n = io::copy(&mut reader, &mut encoder)
            .map_err(|e| PdsError::Internal(format!("Failed to write backup: {}", e)))?;
        encoder
            .finish()
            .map_err(|e| PdsError::Internal(format!("Failed to finalize gzip: {}", e)))?;
        n
    } else {
        let mut writer = io::BufWriter::new(output_file);
        io::copy(&mut reader, &mut writer)
            .map_err(|e| PdsError::Internal(format!("Failed to write backup: {}", e)))?
    };

    // Wait for pg_dump to exit. If it failed, surface the stderr so
    // operators can see the actual cause rather than just an exit code.
    let output = dump.wait_with_output().map_err(|e| {
        PdsError::Internal(format!("Failed to wait for pg_dump: {}", e))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Try to remove the half-written backup file so operators
        // don't mistake it for a usable backup.
        let _ = fs::remove_file(&output_path);
        return Err(PdsError::Internal(format!(
            "pg_dump exited with status {}: {}",
            output.status, stderr.trim()
        )));
    }

    println!(
        "✓ Backup complete: {}",
        format_size(bytes_written)
    );
    println!("\n════════════════════════════════════════════════════════");
    println!("✅ Backup completed successfully");
    println!("════════════════════════════════════════════════════════\n");
    Ok(())
}

/// Postgres restore: pre-flight checks, then loads the backup via
/// `psql`. Pre-flight tries to acquire the sequencer leader advisory
/// lock — if it fails, another aurora-locus instance is live against
/// this database, and restoring would race; we warn and require
/// `--yes` to proceed.
async fn restore_postgres(
    ctx: &AppContext,
    input: &str,
    skip_confirmation: bool,
) -> PdsResult<()> {
    use crate::sequencer::{PostgresLockProvider, SEQUENCER_LEADER_LOCK_KEY};
    use crate::sequencer::leader_election::LockProvider;

    println!("════════════════════════════════════════════════════════");
    println!("  Database Restore (Postgres)");
    println!("════════════════════════════════════════════════════════");

    let url = postgres_url(ctx)?;
    let input_path = Path::new(input);
    println!("Source: {}", input_path.display());
    println!("Target: {} (Postgres)", redact_url(&url));

    if !input_path.exists() {
        return Err(PdsError::NotFound(format!(
            "Backup file not found: {}",
            input_path.display()
        )));
    }

    let is_compressed = input_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "gz")
        .unwrap_or(false);
    println!(
        "Format: {}",
        if is_compressed { "gzip-compressed SQL" } else { "plain SQL" }
    );

    // Pre-flight: check whether the sequencer leader advisory lock is
    // held. If it is, an aurora-locus instance is live against this
    // database; restoring would race with its writes.
    println!("\n🔍 Pre-flight: checking for active aurora-locus instances...");
    let provider = PostgresLockProvider::new(
        ctx.account_db.clone(),
        SEQUENCER_LEADER_LOCK_KEY,
    );
    let lock_was_free = provider.try_acquire().await;
    if lock_was_free {
        // Release immediately — we acquired only to probe.
        provider.release().await;
        println!("✓ No active aurora-locus instances detected.");
    } else {
        println!(
            "⚠️  WARNING: Sequencer leader lock is held — another aurora-locus instance \
             may be running against this database. Restoring while writes are in flight \
             will produce an inconsistent state."
        );
        if !skip_confirmation {
            print!("Proceed anyway? [y/N]: ");
            io::stdout().flush().unwrap();
            let mut response = String::new();
            io::stdin()
                .read_line(&mut response)
                .map_err(|e| PdsError::Internal(format!("read user input: {}", e)))?;
            if !response.trim().eq_ignore_ascii_case("y") {
                println!("Restore cancelled.");
                return Ok(());
            }
        }
    }

    if !skip_confirmation {
        println!("\n⚠️  WARNING: This will overwrite the current database state!");
        print!("Proceed with restore? [y/N]: ");
        io::stdout().flush().unwrap();
        let mut response = String::new();
        io::stdin()
            .read_line(&mut response)
            .map_err(|e| PdsError::Internal(format!("read user input: {}", e)))?;
        if !response.trim().eq_ignore_ascii_case("y") {
            println!("Restore cancelled.");
            return Ok(());
        }
    }

    // Open input, optionally decompress, pipe to psql.
    println!("\n📦 Running psql...");
    let input_file = File::open(input_path).map_err(|e| {
        PdsError::Internal(format!(
            "Failed to open backup {}: {}",
            input_path.display(),
            e
        ))
    })?;

    let mut psql = Command::new("psql")
        .arg("--quiet")
        .arg("--single-transaction")
        .arg("--set=ON_ERROR_STOP=on")
        .arg(&url)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            PdsError::Internal(format!(
                "Failed to invoke psql (is it installed and on PATH?): {}",
                e
            ))
        })?;

    let mut psql_stdin = psql.stdin.take().expect("psql stdin piped");
    if is_compressed {
        let mut decoder = GzDecoder::new(input_file);
        io::copy(&mut decoder, &mut psql_stdin)
            .map_err(|e| PdsError::Internal(format!("Failed to feed psql: {}", e)))?;
    } else {
        let mut reader = io::BufReader::new(input_file);
        io::copy(&mut reader, &mut psql_stdin)
            .map_err(|e| PdsError::Internal(format!("Failed to feed psql: {}", e)))?;
    }
    drop(psql_stdin);

    let status = psql.wait().map_err(|e| {
        PdsError::Internal(format!("Failed to wait for psql: {}", e))
    })?;
    if !status.success() {
        return Err(PdsError::Internal(format!(
            "psql exited with status {} — restore likely incomplete; \
             check the database state manually",
            status
        )));
    }

    // Post-flight: confirm the schema is recognizably Aurora-Locus.
    // Run a basic SELECT against a known table; if it fails, the
    // restore landed in an unexpected state.
    println!("\n🔍 Post-flight: verifying restored schema...");
    let smoke: Result<i64, _> =
        sqlx::query_scalar("SELECT COUNT(*) FROM actor")
            .fetch_one(&ctx.account_db)
            .await;
    match smoke {
        Ok(n) => println!("✓ Schema OK; {} accounts present.", n),
        Err(e) => {
            return Err(PdsError::Internal(format!(
                "Post-flight check failed: SELECT COUNT(*) FROM actor returned {} — \
                 restore landed in an unexpected state",
                e
            )));
        }
    }

    println!("\n════════════════════════════════════════════════════════");
    println!("✅ Restore completed successfully");
    println!("════════════════════════════════════════════════════════\n");
    Ok(())
}

/// Extract the Postgres URL from config; fail loudly if backend is
/// Postgres but URL is unset (shouldn't happen — config validation
/// rejects this — but defensive).
fn postgres_url(ctx: &AppContext) -> PdsResult<String> {
    ctx.config.database.url.clone().ok_or_else(|| {
        PdsError::Validation(
            "PDS_DB_URL is not set — required for Postgres backup/restore".to_string(),
        )
    })
}

/// Hide the password component of a Postgres URL for log output.
/// `postgres://user:secret@host/db` → `postgres://user:***@host/db`.
fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((userpass, hostpath)) = rest.split_once('@') {
            if let Some((user, _pass)) = userpass.split_once(':') {
                return format!("{}://{}:***@{}", scheme, user, hostpath);
            }
        }
    }
    url.to_string()
}

/// Format file size for human-readable output
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
