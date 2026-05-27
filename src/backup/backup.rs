//! Online database backup using SQLite's backup API.

use crate::error::{PoolError, Result};
use crate::pool::DatabasePool;
use std::path::Path;
use std::time::Duration;

/// Configuration for database backups.
#[derive(Debug, Clone)]
pub struct BackupConfig {
    /// Number of pages to copy per step (default: 100).
    /// Smaller values are more incremental but slower.
    pub pages_per_step: i32,

    /// Sleep duration between steps (default: 10ms).
    /// Allows other operations to proceed during backup.
    pub step_sleep: Duration,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            pages_per_step: 100,
            step_sleep: Duration::from_millis(10),
        }
    }
}

impl BackupConfig {
    /// Create a fast backup config (larger steps, no sleep).
    /// Best for small databases or when speed is critical.
    pub fn fast() -> Self {
        Self {
            pages_per_step: 1000,
            step_sleep: Duration::ZERO,
        }
    }

    /// Create a gentle backup config (smaller steps, longer sleep).
    /// Best for large databases under heavy load.
    pub fn gentle() -> Self {
        Self {
            pages_per_step: 50,
            step_sleep: Duration::from_millis(50),
        }
    }
}

/// Progress information during backup.
#[derive(Debug, Clone)]
pub struct BackupProgress {
    /// Pages remaining to copy.
    pub remaining: i32,
    /// Total pages in database.
    pub total: i32,
}

impl BackupProgress {
    /// Get completion percentage (0.0 to 100.0).
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            ((self.total - self.remaining) as f64 / self.total as f64) * 100.0
        }
    }

    /// Check if backup is complete.
    pub fn is_complete(&self) -> bool {
        self.remaining == 0
    }
}

/// Backup the database to a file.
///
/// Uses SQLite's online backup API, which allows backup while the database
/// is in use. The backup is atomic - it either completes fully or not at all.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::backup::backup_database;
///
/// # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
/// backup_database(pool, "backup.db").await?;
/// # Ok(())
/// # }
/// ```
pub async fn backup_database<P: AsRef<Path>>(pool: &DatabasePool, dest: P) -> Result<()> {
    backup_database_with_config(pool, dest, BackupConfig::default(), |_| {}).await
}

/// Backup the database with progress callback.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::backup::backup_database_with_progress;
///
/// # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
/// backup_database_with_progress(pool, "backup.db", |progress| {
///     println!("Backup: {:.1}% complete", progress.percent());
/// }).await?;
/// # Ok(())
/// # }
/// ```
pub async fn backup_database_with_progress<P, F>(
    pool: &DatabasePool,
    dest: P,
    on_progress: F,
) -> Result<()>
where
    P: AsRef<Path>,
    F: Fn(BackupProgress) + Send + 'static,
{
    backup_database_with_config(pool, dest, BackupConfig::default(), on_progress).await
}

/// Backup with full configuration control.
pub async fn backup_database_with_config<P, F>(
    pool: &DatabasePool,
    dest: P,
    config: BackupConfig,
    on_progress: F,
) -> Result<()>
where
    P: AsRef<Path>,
    F: Fn(BackupProgress) + Send + 'static,
{
    let dest_path = dest.as_ref().to_path_buf();

    pool.read(move |conn| {
        // Open destination database
        let mut dest_conn = rusqlite::Connection::open(&dest_path)?;

        // Initialize backup (from source to dest)
        let backup = rusqlite::backup::Backup::new(conn, &mut dest_conn)?;

        loop {
            // Step the backup
            let result = backup.step(config.pages_per_step)?;

            // Report progress using backup info
            let progress = BackupProgress {
                remaining: backup.progress().remaining,
                total: backup.progress().pagecount,
            };
            on_progress(progress);

            match result {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => {
                    // Sleep between steps to allow other operations
                    if !config.step_sleep.is_zero() {
                        std::thread::sleep(config.step_sleep);
                    }
                }
                rusqlite::backup::StepResult::Busy => {
                    // Database is busy, sleep and retry
                    std::thread::sleep(Duration::from_millis(10));
                }
                rusqlite::backup::StepResult::Locked => {
                    // Database is locked, sleep and retry
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    // Handle any future variants
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| PoolError::Backup(format!("backup failed: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_backup_database() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let backup_path = dir.path().join("backup.db");

        let pool = DatabasePool::open(&db_path, crate::PoolConfig::minimal())
            .await
            .unwrap();

        // Create some data
        pool.write(|conn| {
            conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])?;
            conn.execute("INSERT INTO test (value) VALUES ('hello')", [])?;
            Ok(())
        })
        .await
        .unwrap();

        // Backup
        backup_database(&pool, &backup_path).await.unwrap();

        // Verify backup
        let backup_conn = rusqlite::Connection::open(&backup_path).unwrap();
        let value: String = backup_conn
            .query_row("SELECT value FROM test WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "hello");

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_backup_with_progress() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let backup_path = dir.path().join("backup.db");

        let pool = DatabasePool::open(&db_path, crate::PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
            .await
            .unwrap();

        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        backup_database_with_progress(&pool, &backup_path, move |_progress| {
            called_clone.store(true, Ordering::SeqCst);
        })
        .await
        .unwrap();

        assert!(called.load(Ordering::SeqCst));

        pool.close().await.unwrap();
    }
}
