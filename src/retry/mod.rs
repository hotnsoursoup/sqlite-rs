//! Retry and resilience utilities.
//!
//! Provides automatic retry with exponential backoff for handling transient
//! SQLite errors like SQLITE_BUSY or SQLITE_LOCKED.
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_rs::retry::{RetryConfig, with_retry};
//!
//! # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_rs::PoolError> {
//! // Retry with default config (3 attempts, exponential backoff)
//! let result = with_retry(RetryConfig::default(), || {
//!     conn.execute("UPDATE counters SET value = value + 1 WHERE id = 1", [])
//! })?;
//! # Ok(())
//! # }
//! ```

mod config;
mod executor;

pub use config::{RetryConfig, RetryStrategy};
pub use executor::{is_retryable_error, with_retry, with_retry_async};
