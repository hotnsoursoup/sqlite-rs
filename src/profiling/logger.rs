//! Slow query logging.

use super::stats::QueryStats;
use std::panic;
use std::sync::Arc;
use std::time::Duration;

/// Callback function type for slow query notifications.
///
/// # Panic Safety
///
/// Callbacks are invoked within `catch_unwind`, so panics are caught and logged
/// rather than propagating up the call stack.
pub type SlowQueryCallback = Arc<dyn Fn(&QueryStats) + Send + Sync>;

/// A logger for slow queries.
///
/// Calls a callback whenever a query exceeds the configured threshold.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::profiling::{SlowQueryLogger, QueryStats};
/// use std::time::Duration;
///
/// let logger = SlowQueryLogger::new(Duration::from_millis(100))
///     .with_callback(|stats| {
///         eprintln!("Slow query: {} took {:?}", stats.sql, stats.duration);
///     });
///
/// // This will trigger the callback
/// logger.log(&QueryStats::new(
///     "SELECT * FROM big_table",
///     Duration::from_millis(500),
///     1000,
/// ));
/// ```
#[derive(Clone)]
pub struct SlowQueryLogger {
    threshold: Duration,
    callback: Option<SlowQueryCallback>,
}

impl SlowQueryLogger {
    /// Create a new slow query logger with the given threshold.
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            callback: None,
        }
    }

    /// Set the callback function for slow queries.
    ///
    /// # Panic Safety
    ///
    /// The callback is invoked within `catch_unwind`. If it panics, the panic
    /// is caught and an error is logged, but the logger continues to function.
    pub fn with_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(&QueryStats) + Send + Sync + 'static,
    {
        self.callback = Some(Arc::new(callback));
        self
    }

    /// Get the slow query threshold.
    pub fn threshold(&self) -> Duration {
        self.threshold
    }

    /// Log a query if it exceeds the threshold.
    ///
    /// Returns `true` if the query was slow (and logged), `false` otherwise.
    pub fn log(&self, stats: &QueryStats) -> bool {
        if stats.is_slow(self.threshold) {
            // Use tracing if available - use semantic field names per observability guidelines
            crate::telemetry::warn!(
                db_statement = %stats.sql,
                db_operation = "query",
                duration_ms = stats.duration.as_millis(),
                rows_affected = stats.rows_affected,
                "slow query detected"
            );

            // Call custom callback if set, with panic safety
            if let Some(ref callback) = self.callback {
                self.invoke_callback_safely(callback, stats);
            }

            true
        } else {
            false
        }
    }

    /// Invoke callback with panic protection.
    fn invoke_callback_safely(&self, callback: &SlowQueryCallback, stats: &QueryStats) {
        // Clone stats for the closure since catch_unwind requires 'static
        let stats_clone = stats.clone();
        let callback_clone = Arc::clone(callback);

        let result = panic::catch_unwind(panic::AssertUnwindSafe(move || {
            callback_clone(&stats_clone);
        }));

        if let Err(_panic_info) = result {
            crate::telemetry::error!(
                db_statement = %stats.sql,
                "slow query callback panicked"
            );
        }
    }

    /// Create a logger that logs to stderr.
    pub fn stderr(threshold: Duration) -> Self {
        Self::new(threshold).with_callback(|stats| {
            eprintln!(
                "[SLOW QUERY] {} ({:?}, {} rows)",
                stats.sql, stats.duration, stats.rows_affected
            );
        })
    }
}

impl Default for SlowQueryLogger {
    fn default() -> Self {
        Self::new(Duration::from_millis(100))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    #[test]
    fn test_slow_query_detected() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let logger = SlowQueryLogger::new(Duration::from_millis(100)).with_callback(move |_| {
            called_clone.store(true, Ordering::SeqCst);
        });

        // Fast query - should not trigger
        let fast = QueryStats::new("SELECT 1", Duration::from_millis(10), 1);
        assert!(!logger.log(&fast));
        assert!(!called.load(Ordering::SeqCst));

        // Slow query - should trigger
        let slow = QueryStats::new("SELECT * FROM big", Duration::from_millis(500), 1000);
        assert!(logger.log(&slow));
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_callback_panic_safety() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let logger = SlowQueryLogger::new(Duration::from_millis(100)).with_callback(move |_| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            panic!("intentional panic for testing");
        });

        // First slow query - callback panics but is caught
        let slow1 = QueryStats::new("SELECT 1", Duration::from_millis(500), 1);
        let result1 = logger.log(&slow1);

        // Logger should still return true (query was slow)
        assert!(result1);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second slow query - logger should still work
        let slow2 = QueryStats::new("SELECT 2", Duration::from_millis(500), 1);
        let result2 = logger.log(&slow2);

        assert!(result2);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_threshold_boundary() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let logger = SlowQueryLogger::new(Duration::from_millis(100)).with_callback(move |_| {
            called_clone.store(true, Ordering::SeqCst);
        });

        // Exactly at threshold - should NOT be slow (uses strict >)
        let at_threshold = QueryStats::new("SELECT 1", Duration::from_millis(100), 1);
        assert!(!logger.log(&at_threshold));
        assert!(!called.load(Ordering::SeqCst));

        // Just over threshold - should be slow
        let over_threshold = QueryStats::new("SELECT 1", Duration::from_millis(101), 1);
        assert!(logger.log(&over_threshold));
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_zero_threshold() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let logger = SlowQueryLogger::new(Duration::ZERO).with_callback(move |_| {
            called_clone.store(true, Ordering::SeqCst);
        });

        // Any query with duration > 0 should be slow
        let query = QueryStats::new("SELECT 1", Duration::from_nanos(1), 1);
        assert!(logger.log(&query));
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_stderr_constructor() {
        // Just verify it doesn't panic
        let logger = SlowQueryLogger::stderr(Duration::from_millis(100));
        assert_eq!(logger.threshold(), Duration::from_millis(100));
    }

    #[test]
    fn test_default() {
        let logger = SlowQueryLogger::default();
        assert_eq!(logger.threshold(), Duration::from_millis(100));
    }
}
