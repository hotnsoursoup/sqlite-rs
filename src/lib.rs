//! # sqlite-rs
//!
//! High-performance SQLite connection pool with read/write split architecture,
//! WAL monitoring, migration support, and comprehensive utilities.
//!
//! ## Features
//!
//! - **Read/Write Split**: Dedicated writer connection + concurrent reader pool
//! - **WAL Monitoring**: Background task monitors WAL file size with configurable thresholds
//! - **Migration Framework**: Semantic versioning with baseline + incremental migrations
//! - **Schema Utilities**: Safe column/index additions, schema introspection
//! - **Backup & Recovery**: Online backup, integrity checks, foreign key validation
//! - **Observers**: Unified profiling, metrics, logging, and policy hooks
//! - **Retry Logic**: Exponential backoff for transient errors (SQLITE_BUSY)
//! - **Batch Operations**: Efficient bulk inserts, chunked iteration
//! - **Savepoints**: Nested transaction control within transactions
//! - **Graceful Shutdown**: Signal-based shutdown with WAL checkpoint
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use sqlite_rs::{DatabasePool, PoolConfig, Migration};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), sqlite_rs::PoolError> {
//!     // Open with default config
//!     let pool = DatabasePool::open("data/app.db", PoolConfig::default()).await?;
//!
//!     // Run migrations (use include_str! in real code)
//!     pool.migrate(&[
//!         Migration::baseline("1.0.0", "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);"),
//!         Migration::incremental("1.1.0", "1.0.0", "ALTER TABLE users ADD COLUMN email TEXT;"),
//!     ]).await?;
//!
//!     // Read operations (concurrent)
//!     let result = pool.read(|conn| {
//!         let mut stmt = conn.prepare("SELECT COUNT(*) FROM users")?;
//!         stmt.query_row([], |row| row.get::<_, i64>(0))
//!     }).await?;
//!
//!     // Write operations (serialized)
//!     pool.write(|conn| {
//!         conn.execute("INSERT INTO users (name) VALUES (?)", ["Alice"])
//!     }).await?;
//!
//!     // Transaction support
//!     pool.transaction(|tx| {
//!         tx.execute("UPDATE accounts SET balance = balance - 100 WHERE id = 1", [])?;
//!         tx.execute("UPDATE accounts SET balance = balance + 100 WHERE id = 2", [])?;
//!         Ok(())
//!     }).await?;
//!
//!     // Graceful shutdown
//!     pool.close().await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    DatabasePool                          │
//! ├─────────────────────────────────────────────────────────┤
//! │  ┌─────────────┐    ┌─────────────────────────────────┐ │
//! │  │   Writer    │    │         Reader Pool             │ │
//! │  │  (1 conn)   │    │  ┌───┐ ┌───┐ ┌───┐ ┌───┐       │ │
//! │  │             │    │  │ R │ │ R │ │ R │ │ R │ ...   │ │
//! │  │ Serialized  │    │  └───┘ └───┘ └───┘ └───┘       │ │
//! │  │   writes    │    │      Concurrent reads           │ │
//! │  └─────────────┘    └─────────────────────────────────┘ │
//! ├─────────────────────────────────────────────────────────┤
//! │  WAL Monitor (background task)                          │
//! │  - Checks WAL size periodically                         │
//! │  - Warns when threshold exceeded                        │
//! │  - Can trigger checkpoint                               │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Module Overview
//!
//! - `pool` - Main database pool with read/write split
//! - `migrations` - Schema versioning and migration framework
//! - [`schema`] - Schema introspection and safe DDL utilities
//! - [`backup`] - Online backup and integrity checking
//! - [`observer`] - Unified operation observer hooks
//! - [`profiling`] - Query profiling and slow query logging
//! - [`retry`] - Retry logic with exponential backoff
//! - [`batch`] - Batch insert and chunked iteration utilities
//! - [`maintenance`] - Health checks, vacuum, and optimization
//! - [`write_queue`] - Optional pool-backed write queue
//! - `config` - Pool and WAL configuration
//! - `error` - Error types

mod config;
mod connection;
mod error;
pub mod error_util;
mod migrations;
mod pool;
mod savepoint;
mod sql;
pub mod telemetry;

// Feature modules (public)
pub mod backup;
pub mod batch;
pub mod maintenance;
pub mod observer;
pub mod profiling;
pub mod retry;
pub mod schema;
pub mod write_queue;

#[cfg(feature = "wal-monitor")]
pub(crate) mod wal;

// Main exports
pub use config::{InitHook, PoolConfig, SynchronousMode, WalConfig};
pub use connection::{apply_init_hook, AsyncConnection};
pub use error::PoolError;
pub use pool::{DatabasePool, PoolStats};
pub use savepoint::Savepoint;

// Migration exports - Core types
pub use migrations::{compare_versions, detect_schema_state, get_current_version, SchemaState};
pub use migrations::{Migration, MigrationKind};

// Migration exports - Runner
pub use migrations::{
    get_migration_history, get_pending_migrations, is_migration_applied, run_migrations,
    run_migrations_with_options, MigrationOptions, MigrationRecord, MigrationResult,
};

// Migration exports - Recovery
pub use migrations::{
    drop_all_tables, repair_database, reset_database, reset_database_with_options,
    validate_schema_integrity, validate_table_columns, RepairResult, SchemaIssue,
};

/// Re-export rusqlite types commonly needed by users
pub mod rusqlite {
    pub use rusqlite::{
        params, Connection, Error as SqliteError, Result, Row, Statement, Transaction,
    };
}
