//! Database Backup and Restore CLI Commands
//!
//! Provides command-line tools for backing up and restoring SQLite databases.

use crate::{context::AppContext, error::PdsResult};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Backup database to file
pub async fn backup_database(
    ctx: &AppContext,
    output: &str,
    compress: bool,
    all: bool,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Database Backup");
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

/// Restore database from backup
pub async fn restore_database(
    ctx: &AppContext,
    input: &str,
    skip_confirmation: bool,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Database Restore");
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
