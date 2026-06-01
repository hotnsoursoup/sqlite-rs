//! Query logging observer.

use super::context::OperationContext;
use super::traits::{Observer, ObserverResult};
use std::time::Duration;

/// Observer for logging queries and their durations.
///
/// Logs queries at debug level (if `log_all` is true) and slow queries
/// at warn level (when duration exceeds `slow_threshold_ms`).
///
/// # Example
///
/// ```rust
/// use sqlite_kit::observer::QueryLogger;
///
/// // Default: log slow queries (>100ms)
/// let logger = QueryLogger::new();
///
/// // Log all queries
/// let verbose_logger = QueryLogger::new()
///     .with_log_all(true)
///     .with_slow_threshold_ms(50);
/// ```
#[derive(Debug, Clone)]
pub struct QueryLogger {
    /// Threshold in milliseconds for "slow" queries.
    slow_threshold_ms: u64,
    /// Whether to log all queries (not just slow ones).
    log_all: bool,
    /// Maximum SQL length to log (truncate longer queries).
    max_sql_length: usize,
    /// Prefix for log messages.
    log_prefix: &'static str,
}

impl Default for QueryLogger {
    fn default() -> Self {
        Self {
            slow_threshold_ms: 100,
            log_all: false,
            max_sql_length: 1000,
            log_prefix: "[sqlite-kit]",
        }
    }
}

impl QueryLogger {
    /// Create a new query logger with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the slow query threshold in milliseconds.
    ///
    /// Queries taking longer than this will be logged at warn level.
    pub fn with_slow_threshold_ms(mut self, threshold: u64) -> Self {
        self.slow_threshold_ms = threshold;
        self
    }

    /// Enable or disable logging of all queries.
    ///
    /// When true, all queries are logged at debug level.
    /// When false (default), only slow queries are logged.
    pub fn with_log_all(mut self, log_all: bool) -> Self {
        self.log_all = log_all;
        self
    }

    /// Set the maximum SQL length to log.
    ///
    /// Longer queries will be truncated with "...".
    pub fn with_max_sql_length(mut self, length: usize) -> Self {
        self.max_sql_length = length;
        self
    }

    /// Set a custom log prefix.
    pub fn with_prefix(mut self, prefix: &'static str) -> Self {
        self.log_prefix = prefix;
        self
    }

    /// Truncate SQL for logging (UTF-8 safe).
    fn truncate_sql(&self, sql: &str) -> String {
        if sql.len() <= self.max_sql_length {
            sql.to_string()
        } else {
            // Find a valid UTF-8 char boundary at or before max_sql_length
            let mut end = self.max_sql_length;
            while end > 0 && !sql.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &sql[..end])
        }
    }
}

impl Observer for QueryLogger {
    fn before_operation(&self, ctx: &mut OperationContext) -> ObserverResult<()> {
        if self.log_all {
            let _sql = self.truncate_sql(ctx.sql());
            crate::telemetry::debug!(
                prefix = %self.log_prefix,
                operation = %ctx.operation().as_str(),
                sql = %_sql,
                "executing query"
            );
        }
        Ok(())
    }

    fn after_operation(&self, ctx: &OperationContext, duration: Duration, _success: bool) {
        let duration_ms = duration.as_millis() as u64;
        let is_slow = duration_ms >= self.slow_threshold_ms;

        if is_slow {
            let _sql = self.truncate_sql(ctx.sql());
            crate::telemetry::warn!(
                prefix = %self.log_prefix,
                operation = %ctx.operation().as_str(),
                duration_ms = duration_ms,
                success = _success,
                threshold_ms = self.slow_threshold_ms,
                sql = %_sql,
                "slow query detected"
            );
        } else if self.log_all {
            crate::telemetry::debug!(
                prefix = %self.log_prefix,
                operation = %ctx.operation().as_str(),
                duration_ms = duration_ms,
                success = _success,
                "query completed"
            );
        }
    }

    fn name(&self) -> &'static str {
        "QueryLogger"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logger_defaults() {
        let logger = QueryLogger::new();
        assert_eq!(logger.slow_threshold_ms, 100);
        assert!(!logger.log_all);
        assert_eq!(logger.max_sql_length, 1000);
    }

    #[test]
    fn test_logger_builder() {
        let logger = QueryLogger::new()
            .with_slow_threshold_ms(50)
            .with_log_all(true)
            .with_max_sql_length(500)
            .with_prefix("[test]");

        assert_eq!(logger.slow_threshold_ms, 50);
        assert!(logger.log_all);
        assert_eq!(logger.max_sql_length, 500);
        assert_eq!(logger.log_prefix, "[test]");
    }

    #[test]
    fn test_truncate_sql() {
        let logger = QueryLogger::new().with_max_sql_length(10);

        assert_eq!(logger.truncate_sql("short"), "short");
        assert_eq!(logger.truncate_sql("0123456789"), "0123456789");
        assert_eq!(logger.truncate_sql("0123456789X"), "0123456789...");
    }

    #[test]
    fn test_truncate_sql_utf8() {
        // Test with multi-byte UTF-8 characters (each emoji is 4 bytes)
        let logger = QueryLogger::new().with_max_sql_length(5);

        // "Hello" is 5 bytes, fits exactly
        assert_eq!(logger.truncate_sql("Hello"), "Hello");

        // "Hello!" is 6 bytes, truncates to "Hello..."
        assert_eq!(logger.truncate_sql("Hello!"), "Hello...");

        // Test truncation at multi-byte boundary doesn't panic
        // "🎉" is 4 bytes, setting max to 2 should not panic
        let logger2 = QueryLogger::new().with_max_sql_length(2);
        let result = logger2.truncate_sql("🎉test");
        // Should truncate before the emoji (at byte 0 since 2 is mid-emoji)
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_logger_name() {
        let logger = QueryLogger::new();
        assert_eq!(logger.name(), "QueryLogger");
    }

    #[test]
    fn test_before_operation_allows() {
        let logger = QueryLogger::new();
        let mut ctx = OperationContext::new("SELECT 1");
        assert!(logger.before_operation(&mut ctx).is_ok());
    }
}
