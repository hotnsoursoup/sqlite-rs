//! Error types for the connection pool.

use thiserror::Error;

/// Errors that can occur when using the database pool.
#[derive(Debug, Error)]
pub enum PoolError {
    /// Failed to open the database file.
    #[error("failed to open database: {0}")]
    Open(String),

    /// Failed to open the dedicated writer connection.
    #[error("failed to open writer connection: {0}")]
    WriterOpen(String),

    /// Failed to create the reader pool.
    #[error("failed to create reader pool: {0}")]
    PoolCreate(String),

    /// Failed to get a reader connection from the pool.
    #[error("failed to get reader from pool: {0}")]
    PoolGet(String),

    /// Pool is closed.
    #[error("pool is closed")]
    PoolClosed,

    /// Error during async interaction with the database.
    #[error("database interaction error: {0}")]
    Interact(String),

    /// SQLite error.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Async SQLite bridge error.
    #[error("async SQLite error: {0}")]
    AsyncSqlite(#[from] tokio_rusqlite::Error),

    /// Migration error.
    #[error("migration error: {0}")]
    Migration(String),

    /// Schema validation error.
    #[error("schema error: {0}")]
    Schema(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Config(String),

    /// I/O error (file operations).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Timeout waiting for connection.
    #[error("timeout waiting for connection: {0}")]
    Timeout(String),

    /// Backup error.
    #[error("backup error: {0}")]
    Backup(String),

    /// Integrity check error.
    #[error("integrity error: {0}")]
    Integrity(String),

    /// Retry exhausted.
    #[error("retry exhausted after {attempts} attempts: {last_error}")]
    RetryExhausted { attempts: u32, last_error: String },

    /// Batch operation error.
    #[error("batch error: {0}")]
    Batch(String),

    /// Maintenance operation error.
    #[error("maintenance error: {0}")]
    Maintenance(String),

    /// Savepoint operation error.
    #[error("savepoint error: {0}")]
    Savepoint(String),

    /// Observer rejected or failed an operation.
    #[error("observer error: {0}")]
    Observer(String),
}

/// Result type alias for pool operations.
pub type Result<T> = std::result::Result<T, PoolError>;
