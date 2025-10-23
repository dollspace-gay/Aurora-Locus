/// Automated backup scheduling and management for Aurora Locus PDS
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{error, info, warn};

/// Backup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Enable automated backups
    pub enabled: bool,

    /// Backup interval in hours (default: 24 hours = daily)
    pub interval_hours: u64,

    /// Backup directory path
    pub backup_dir: PathBuf,

    /// Number of days to retain backups (default: 30)
    pub retain_days: u32,

    /// Compression type: "gzip", "bzip2", "xz", "none"
    pub compression: String,

    /// Path to backup script
    pub script_path: Option<PathBuf>,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: 24,
            backup_dir: PathBuf::from("./backups"),
            retain_days: 30,
            compression: "gzip".to_string(),
            script_path: None,
        }
    }
}

impl BackupConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("BACKUP_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            interval_hours: std::env::var("BACKUP_INTERVAL_HOURS")
                .unwrap_or_else(|_| "24".to_string())
                .parse()
                .unwrap_or(24),
            backup_dir: std::env::var("BACKUP_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./backups")),
            retain_days: std::env::var("BACKUP_RETAIN_DAYS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap_or(30),
            compression: std::env::var("BACKUP_COMPRESSION")
                .unwrap_or_else(|_| "gzip".to_string()),
            script_path: std::env::var("BACKUP_SCRIPT_PATH")
                .ok()
                .map(PathBuf::from),
        }
    }
}

/// Backup scheduler manages automated backups
pub struct BackupScheduler {
    config: BackupConfig,
    last_backup: Option<DateTime<Utc>>,
}

impl BackupScheduler {
    /// Create a new backup scheduler
    pub fn new(config: BackupConfig) -> Self {
        Self {
            config,
            last_backup: None,
        }
    }

    /// Start the backup scheduler (runs in background)
    pub async fn start(mut self) -> PdsResult<()> {
        if !self.config.enabled {
            info!("Automated backups are disabled");
            return Ok(());
        }

        info!(
            "Starting backup scheduler (interval: {} hours, retention: {} days)",
            self.config.interval_hours, self.config.retain_days
        );

        let interval_duration = TokioDuration::from_secs(self.config.interval_hours * 3600);
        let mut ticker = interval(interval_duration);

        loop {
            ticker.tick().await;

            info!("Running scheduled backup...");
            match self.run_backup().await {
                Ok(backup_path) => {
                    self.last_backup = Some(Utc::now());
                    info!("✓ Scheduled backup completed: {:?}", backup_path);
                }
                Err(e) => {
                    error!("✗ Scheduled backup failed: {}", e);
                }
            }
        }
    }

    /// Run a backup manually
    pub async fn run_backup(&self) -> PdsResult<PathBuf> {
        info!("Starting backup process...");

        // Determine script path
        let script_path = if let Some(ref path) = self.config.script_path {
            path.clone()
        } else {
            // Default to scripts/backup.sh or scripts/backup.ps1 depending on OS
            let project_root = std::env::current_dir()
                .map_err(|e| PdsError::Internal(format!("Failed to get current dir: {}", e)))?;

            if cfg!(windows) {
                project_root.join("scripts").join("backup.ps1")
            } else {
                project_root.join("scripts").join("backup.sh")
            }
        };

        if !script_path.exists() {
            return Err(PdsError::Internal(format!(
                "Backup script not found: {:?}",
                script_path
            )));
        }

        // Run backup script
        let output = if cfg!(windows) {
            Command::new("powershell")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(&script_path)
                .arg("-BackupDir")
                .arg(&self.config.backup_dir)
                .output()
        } else {
            Command::new("bash")
                .arg(&script_path)
                .arg(&self.config.backup_dir)
                .output()
        }
        .map_err(|e| PdsError::Internal(format!("Failed to execute backup script: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PdsError::Internal(format!(
                "Backup script failed: {}",
                stderr
            )));
        }

        // Parse output to get backup path (last line should contain it)
        let stdout = String::from_utf8_lossy(&output.stdout);
        info!("Backup output:\n{}", stdout);

        // Return the backup directory
        Ok(self.config.backup_dir.clone())
    }

    /// Get the last backup time
    pub fn last_backup_time(&self) -> Option<DateTime<Utc>> {
        self.last_backup
    }

    /// Check if a backup is due
    pub fn is_backup_due(&self) -> bool {
        if !self.config.enabled {
            return false;
        }

        match self.last_backup {
            Some(last) => {
                let elapsed = Utc::now().signed_duration_since(last);
                let interval = Duration::hours(self.config.interval_hours as i64);
                elapsed >= interval
            }
            None => true, // No backup yet, so it's due
        }
    }
}

/// Backup metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub timestamp: DateTime<Utc>,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub compression: String,
    pub databases: Vec<String>,
}

/// List available backups in the backup directory
pub fn list_backups(backup_dir: &Path) -> PdsResult<Vec<BackupMetadata>> {
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();

    for entry in std::fs::read_dir(backup_dir)
        .map_err(|e| PdsError::Internal(format!("Failed to read backup dir: {}", e)))?
    {
        let entry = entry
            .map_err(|e| PdsError::Internal(format!("Failed to read dir entry: {}", e)))?;
        let path = entry.path();

        if path.is_dir() && path.file_name().unwrap_or_default().to_string_lossy().starts_with("backup_") {
            // Try to read manifest
            let manifest_path = path.join("manifest.json");
            if manifest_path.exists() {
                match std::fs::read_to_string(&manifest_path) {
                    Ok(content) => {
                        if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) {
                            let timestamp = manifest["backup_timestamp"]
                                .as_str()
                                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                                .map(|dt| dt.with_timezone(&Utc))
                                .unwrap_or_else(|| Utc::now());

                            let size_bytes = get_dir_size(&path).unwrap_or(0);

                            backups.push(BackupMetadata {
                                timestamp,
                                path,
                                size_bytes,
                                compression: manifest["compression"]
                                    .as_str()
                                    .unwrap_or("unknown")
                                    .to_string(),
                                databases: vec![
                                    "account".to_string(),
                                    "sequencer".to_string(),
                                    "did_cache".to_string(),
                                ],
                            });
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read manifest for {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    // Sort by timestamp (newest first)
    backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(backups)
}

/// Get the total size of a directory
fn get_dir_size(path: &Path) -> Result<u64, std::io::Error> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += get_dir_size(&entry.path())?;
        } else {
            total += metadata.len();
        }
    }
    Ok(total)
}

/// Delete old backups based on retention policy
pub fn cleanup_old_backups(backup_dir: &Path, retain_days: u32) -> PdsResult<usize> {
    let cutoff = Utc::now() - Duration::days(retain_days as i64);
    let mut deleted_count = 0;

    for backup in list_backups(backup_dir)? {
        if backup.timestamp < cutoff {
            info!(
                "Deleting old backup: {:?} (created: {})",
                backup.path, backup.timestamp
            );

            match std::fs::remove_dir_all(&backup.path) {
                Ok(_) => {
                    deleted_count += 1;
                    info!("  ✓ Deleted");
                }
                Err(e) => {
                    error!("  ✗ Failed to delete: {}", e);
                }
            }
        }
    }

    Ok(deleted_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_config_default() {
        let config = BackupConfig::default();
        assert_eq!(config.enabled, false);
        assert_eq!(config.interval_hours, 24);
        assert_eq!(config.retain_days, 30);
        assert_eq!(config.compression, "gzip");
    }

    #[test]
    fn test_backup_scheduler_creation() {
        let config = BackupConfig::default();
        let scheduler = BackupScheduler::new(config);
        assert!(scheduler.last_backup.is_none());
    }

    #[test]
    fn test_is_backup_due_no_previous_backup() {
        let mut config = BackupConfig::default();
        config.enabled = true;
        let scheduler = BackupScheduler::new(config);
        assert!(scheduler.is_backup_due());
    }

    #[test]
    fn test_is_backup_due_disabled() {
        let config = BackupConfig::default(); // disabled by default
        let scheduler = BackupScheduler::new(config);
        assert!(!scheduler.is_backup_due());
    }

    #[test]
    fn test_list_backups_nonexistent_dir() {
        let result = list_backups(Path::new("/nonexistent/path"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }
}
