//! Retry execution logic.

use super::config::RetryConfig;
use crate::error::{PoolError, Result};
use std::time::Instant;

/// Check if a rusqlite error is retryable.
///
/// Returns `true` for transient errors like SQLITE_BUSY, SQLITE_LOCKED.
pub fn is_retryable_error(error: &rusqlite::Error) -> bool {
    use rusqlite::ffi;

    match error {
        rusqlite::Error::SqliteFailure(err, _) => {
            // Check extended code first for BUSY_SNAPSHOT
            if err.extended_code == ffi::SQLITE_BUSY_SNAPSHOT {
                return true;
            }
            // Check primary error codes
            matches!(
                err.code,
                rusqlite::ErrorCode::DatabaseBusy
                    | rusqlite::ErrorCode::DatabaseLocked
                    | rusqlite::ErrorCode::OperationInterrupted
            )
        }
        _ => false,
    }
}

/// Execute a function with retry logic.
///
/// Automatically retries on transient SQLite errors using the configured
/// retry strategy.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::retry::{with_retry, RetryConfig};
///
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// let rows = with_retry(RetryConfig::default(), || {
///     conn.execute("UPDATE counters SET value = value + 1", [])
/// })?;
/// # Ok(())
/// # }
/// ```
pub fn with_retry<F, T>(config: RetryConfig, mut f: F) -> Result<T>
where
    F: FnMut() -> std::result::Result<T, rusqlite::Error>,
{
    let start = Instant::now();
    let mut last_error = None;

    for attempt in 0..config.max_attempts {
        // Check total timeout
        if !config.total_timeout.is_zero() && start.elapsed() >= config.total_timeout {
            break;
        }

        match f() {
            Ok(result) => return Ok(result),
            Err(e) if is_retryable_error(&e) => {
                crate::telemetry::debug!(
                    attempt = attempt + 1,
                    max_attempts = config.max_attempts,
                    error = %crate::telemetry::chain(&e),
                    "retrying after transient error"
                );

                last_error = Some(e);

                // Sleep before retry (except on last attempt)
                if attempt + 1 < config.max_attempts {
                    let delay = config.delay_for_attempt(attempt);
                    std::thread::sleep(delay);
                }
            }
            Err(e) => {
                // Non-retryable error, fail immediately
                return Err(PoolError::from(e));
            }
        }
    }

    Err(PoolError::RetryExhausted {
        attempts: config.max_attempts,
        last_error: last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string()),
    })
}

/// Async version of retry logic.
///
/// Uses tokio::time::sleep for async-friendly delays.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::retry::{with_retry_async, RetryConfig};
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// let result = with_retry_async(RetryConfig::default(), || async {
///     pool.write(|conn| {
///         conn.execute("UPDATE counters SET value = value + 1", [])
///     }).await
/// }).await?;
/// # Ok(())
/// # }
/// ```
pub async fn with_retry_async<F, Fut, T>(config: RetryConfig, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let start = Instant::now();
    let mut last_error = None;

    for attempt in 0..config.max_attempts {
        // Check total timeout
        if !config.total_timeout.is_zero() && start.elapsed() >= config.total_timeout {
            break;
        }

        match f().await {
            Ok(result) => return Ok(result),
            Err(PoolError::Sqlite(ref e)) if is_retryable_error(e) => {
                crate::telemetry::debug!(
                    attempt = attempt + 1,
                    max_attempts = config.max_attempts,
                    "retrying after transient error"
                );

                last_error = Some(e.to_string());

                // Sleep before retry (except on last attempt)
                if attempt + 1 < config.max_attempts {
                    let delay = config.delay_for_attempt(attempt);
                    tokio::time::sleep(delay).await;
                }
            }
            Err(e) => {
                // Check if it's a wrapped sqlite error that's retryable
                if let PoolError::AsyncSqlite(tokio_rusqlite::Error::Rusqlite(re)) = &e {
                    if is_retryable_error(re) {
                        last_error = Some(re.to_string());
                        if attempt + 1 < config.max_attempts {
                            let delay = config.delay_for_attempt(attempt);
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                }
                // Non-retryable error
                return Err(e);
            }
        }
    }

    Err(PoolError::RetryExhausted {
        attempts: config.max_attempts,
        last_error: last_error.unwrap_or_else(|| "unknown error".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_retry_succeeds_eventually() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(RetryConfig::new(3), move || {
            let n = attempts_clone.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some("database is locked".to_string()),
                ))
            } else {
                Ok(42)
            }
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_retry_exhausted() {
        let result = with_retry(RetryConfig::new(3), || {
            Err::<(), _>(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("database is locked".to_string()),
            ))
        });

        assert!(matches!(
            result,
            Err(PoolError::RetryExhausted { attempts: 3, .. })
        ));
    }

    #[test]
    fn test_non_retryable_error_fails_immediately() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(RetryConfig::new(3), move || {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some("constraint violation".to_string()),
            ))
        });

        assert!(result.is_err());
        // Should only try once since error is not retryable
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
