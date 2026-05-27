//! Tracing spans for database operations. No-ops without the `tracing` feature.

#[cfg(feature = "tracing")]
use super::sql::SqlStatement;

/// A wrapper for database operation spans.
///
/// Provides structured spans for database operations. When the `tracing` feature
/// is disabled, all methods are no-ops.
///
/// # Example
///
/// ```ignore
/// let span = DbSpan::query_execute("select", sql);
/// let _guard = span.enter();
/// // ... execute query ...
/// if let Err(ref e) = result {
///     span.record_error(e);
/// }
/// ```
pub struct DbSpan {
    #[cfg(feature = "tracing")]
    inner: tracing::Span,
    #[cfg(not(feature = "tracing"))]
    _marker: std::marker::PhantomData<()>,
}

impl DbSpan {
    /// Create a span for a query execution.
    ///
    /// # Arguments
    ///
    /// * `operation` - The type of operation (e.g., "select", "insert", "update")
    /// * `sql` - The SQL statement being executed
    #[cfg(feature = "tracing")]
    pub fn query_execute(operation: &str, sql: &str) -> Self {
        let span = tracing::info_span!(
            "db.query",
            db_system = "sqlite",
            db_operation = %operation,
            db_statement = %SqlStatement::new(sql),
            otel.status_code = tracing::field::Empty,
            error.message = tracing::field::Empty,
        );
        Self { inner: span }
    }

    /// Create a span for a query execution (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn query_execute(_operation: &str, _sql: &str) -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Create a span for a database connection.
    #[cfg(feature = "tracing")]
    pub fn connect(db_path: &str) -> Self {
        let span = tracing::info_span!(
            "db.connect",
            db_system = "sqlite",
            db_name = %db_path,
            otel.status_code = tracing::field::Empty,
            error.message = tracing::field::Empty,
        );
        Self { inner: span }
    }

    /// Create a span for a database connection (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn connect(_db_path: &str) -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Create a span for a pool-level operation.
    #[cfg(feature = "tracing")]
    pub fn pool_operation(operation: &str) -> Self {
        let span = tracing::info_span!(
            "db.pool_operation",
            db_system = "sqlite",
            db_operation = %operation,
            otel.status_code = tracing::field::Empty,
            error.message = tracing::field::Empty,
        );
        Self { inner: span }
    }

    /// Create a span for a pool-level operation (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn pool_operation(_operation: &str) -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Create a span for a transaction.
    #[cfg(feature = "tracing")]
    pub fn transaction(operation: &str) -> Self {
        let span = tracing::info_span!(
            "db.transaction",
            db_system = "sqlite",
            db_operation = %operation,
            otel.status_code = tracing::field::Empty,
            error.message = tracing::field::Empty,
        );
        Self { inner: span }
    }

    /// Create a span for a transaction (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn transaction(_operation: &str) -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Enter the span, returning a guard that exits when dropped.
    #[cfg(feature = "tracing")]
    pub fn enter(&self) -> tracing::span::Entered<'_> {
        self.inner.enter()
    }

    /// Enter the span (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn enter(&self) -> DbSpanGuard {
        DbSpanGuard
    }

    /// Record an error on this span.
    #[cfg(feature = "tracing")]
    pub fn record_error(&self, err: &dyn std::error::Error) {
        self.inner.record("otel.status_code", "ERROR");
        // Inline the source chain to dodge lifetime constraints from ErrorChain.
        let mut error_msg = err.to_string();
        let mut source = err.source();
        while let Some(s) = source {
            error_msg.push_str(": ");
            error_msg.push_str(&s.to_string());
            source = s.source();
        }
        self.inner.record("error.message", error_msg.as_str());
    }

    /// Record an error on this span (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn record_error(&self, _err: &dyn std::error::Error) {}

    /// Record success status on this span.
    #[cfg(feature = "tracing")]
    pub fn record_success(&self) {
        self.inner.record("otel.status_code", "OK");
    }

    /// Record success status on this span (no-op when tracing is disabled).
    #[cfg(not(feature = "tracing"))]
    pub fn record_success(&self) {}
}

/// A no-op span guard for when tracing is disabled.
#[cfg(not(feature = "tracing"))]
pub struct DbSpanGuard;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_span_creation() {
        // Just verify it doesn't panic
        let span = DbSpan::query_execute("select", "SELECT 1");
        let _guard = span.enter();
        span.record_success();
    }

    #[test]
    fn test_db_span_error() {
        let span = DbSpan::query_execute("insert", "INSERT INTO t VALUES (1)");
        let _guard = span.enter();

        let err = std::io::Error::other("test error");
        span.record_error(&err);
    }
}
