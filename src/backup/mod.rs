#![allow(clippy::module_inception)]
//! Database backup and integrity utilities.
//!
//! Provides online backup using SQLite's backup API, integrity checking,
//! and database repair utilities.
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_rs::backup::{backup_database, backup_database_with_progress};
//!
//! # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
//! // Simple backup
//! backup_database(pool, "backup.db").await?;
//!
//! // Backup with progress callback
//! backup_database_with_progress(pool, "backup.db", |progress| {
//!     println!("Backup: {:.1}% complete", progress.percent());
//! }).await?;
//! # Ok(())
//! # }
//! ```

mod backup;
mod integrity;

pub use backup::{
    backup_database, backup_database_with_config, backup_database_with_progress, BackupConfig,
    BackupProgress,
};

pub use integrity::{
    foreign_key_check, integrity_check, quick_check, ForeignKeyViolation, IntegrityIssue,
};
