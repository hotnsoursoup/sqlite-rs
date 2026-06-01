//! Query profiler for measuring execution time.

use super::logger::SlowQueryLogger;
use super::stats::{AggregateStats, QueryStats};
use crate::observer::{Observer, OperationContext};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A query profiler that collects execution statistics.
///
/// Can be shared across threads and used to monitor database performance.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::profiling::QueryProfiler;
/// use std::time::Duration;
///
/// let profiler = QueryProfiler::new(Duration::from_millis(100));
///
/// // Record queries
/// profiler.record("SELECT 1", Duration::from_millis(5), 1);
/// profiler.record("SELECT * FROM big", Duration::from_millis(500), 1000);
///
/// // Get aggregate stats
/// let stats = profiler.stats();
/// println!("Total queries: {}", stats.total_queries);
/// println!("Slow queries: {} ({:.1}%)", stats.slow_queries, stats.slow_percentage());
/// ```
#[derive(Clone)]
pub struct QueryProfiler {
    stats: Arc<Mutex<AggregateStats>>,
    logger: SlowQueryLogger,
}

impl QueryProfiler {
    /// Create a new profiler with the given slow query threshold.
    pub fn new(slow_threshold: Duration) -> Self {
        Self {
            stats: Arc::new(Mutex::new(AggregateStats::new())),
            logger: SlowQueryLogger::new(slow_threshold),
        }
    }

    /// Create a profiler with a custom slow query logger.
    pub fn with_logger(logger: SlowQueryLogger) -> Self {
        Self {
            stats: Arc::new(Mutex::new(AggregateStats::new())),
            logger,
        }
    }

    /// Record a query execution.
    pub fn record(&self, sql: impl Into<String>, duration: Duration, rows: u64) {
        let stats = QueryStats::new(sql, duration, rows);
        self.logger.log(&stats);
        self.stats.lock().record(&stats, self.logger.threshold());
    }

    /// Get a snapshot of the current aggregate statistics.
    pub fn stats(&self) -> AggregateStats {
        self.stats.lock().clone()
    }

    /// Reset all statistics.
    pub fn reset(&self) {
        self.stats.lock().reset();
    }

    /// Get the slow query threshold.
    pub fn slow_threshold(&self) -> Duration {
        self.logger.threshold()
    }
}

impl Default for QueryProfiler {
    fn default() -> Self {
        Self::new(Duration::from_millis(100))
    }
}

impl Observer for QueryProfiler {
    fn after_operation(&self, ctx: &OperationContext, duration: Duration, success: bool) {
        self.record(ctx.sql(), duration, u64::from(success));
    }

    fn name(&self) -> &'static str {
        "QueryProfiler"
    }
}

/// A wrapper around a rusqlite Connection that profiles queries.
///
/// Use `execute_profiled` and `query_profiled` to automatically
/// measure and record query execution times.
pub struct ProfiledConnection<'a> {
    conn: &'a rusqlite::Connection,
    profiler: &'a QueryProfiler,
}

impl<'a> ProfiledConnection<'a> {
    /// Wrap a connection with profiling.
    pub fn new(conn: &'a rusqlite::Connection, profiler: &'a QueryProfiler) -> Self {
        Self { conn, profiler }
    }

    /// Get the underlying connection for unprofiled operations.
    pub fn inner(&self) -> &rusqlite::Connection {
        self.conn
    }

    /// Execute a statement with profiling.
    ///
    /// Records the query even if it fails, with rows_affected = 0 on error.
    pub fn execute_profiled(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> rusqlite::Result<usize> {
        let start = Instant::now();
        let result = self.conn.execute(sql, params);
        let duration = start.elapsed();

        let rows = result.as_ref().copied().unwrap_or(0) as u64;
        self.profiler.record(sql, duration, rows);

        result
    }

    /// Execute a batch of statements with profiling.
    ///
    /// Batch operations don't report individual row counts, so rows_affected is always 0.
    pub fn execute_batch_profiled(&self, sql: &str) -> rusqlite::Result<()> {
        let start = Instant::now();
        let result = self.conn.execute_batch(sql);
        let duration = start.elapsed();

        self.profiler.record(sql, duration, 0);

        result
    }

    /// Query a single row with profiling.
    ///
    /// Records rows_affected = 1 on success, 0 on error.
    pub fn query_row_profiled<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let start = Instant::now();
        let result = self.conn.query_row(sql, params, f);
        let duration = start.elapsed();

        self.profiler
            .record(sql, duration, if result.is_ok() { 1 } else { 0 });

        result
    }
}

/// Time a block of code and return the duration along with the result.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::profiling::timed;
///
/// let (duration, result) = timed(|| {
///     // Some operation
///     42
/// });
///
/// println!("Operation took {:?} and returned {}", duration, result);
/// ```
pub fn timed<F, T>(f: F) -> (Duration, T)
where
    F: FnOnce() -> T,
{
    let start = Instant::now();
    let result = f();
    (start.elapsed(), result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profiler_records_stats() {
        let profiler = QueryProfiler::new(Duration::from_millis(100));

        profiler.record("SELECT 1", Duration::from_millis(10), 1);
        profiler.record("SELECT 2", Duration::from_millis(20), 2);
        profiler.record("SELECT * FROM big", Duration::from_millis(500), 1000);

        let stats = profiler.stats();
        assert_eq!(stats.total_queries, 3);
        assert_eq!(stats.slow_queries, 1);
        assert_eq!(stats.total_rows, 1003);
    }

    #[test]
    fn test_profiler_reset() {
        let profiler = QueryProfiler::new(Duration::from_millis(100));
        profiler.record("SELECT 1", Duration::from_millis(10), 1);

        assert_eq!(profiler.stats().total_queries, 1);

        profiler.reset();

        assert_eq!(profiler.stats().total_queries, 0);
    }

    #[test]
    fn test_profiler_default() {
        let profiler = QueryProfiler::default();
        assert_eq!(profiler.slow_threshold(), Duration::from_millis(100));
    }

    #[test]
    fn test_profiler_with_custom_logger() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let logger = SlowQueryLogger::new(Duration::from_millis(50)).with_callback(move |_| {
            called_clone.store(true, Ordering::SeqCst);
        });

        let profiler = QueryProfiler::with_logger(logger);

        // Should trigger with 50ms threshold
        profiler.record("SELECT 1", Duration::from_millis(100), 1);
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_profiler_clone_shares_stats() {
        let profiler1 = QueryProfiler::new(Duration::from_millis(100));
        let profiler2 = profiler1.clone();

        profiler1.record("SELECT 1", Duration::from_millis(10), 1);

        // Both should see the same stats (Arc shared)
        assert_eq!(profiler1.stats().total_queries, 1);
        assert_eq!(profiler2.stats().total_queries, 1);
    }

    #[test]
    fn test_timed() {
        let (duration, result) = timed(|| {
            std::thread::sleep(Duration::from_millis(10));
            42
        });

        assert_eq!(result, 42);
        assert!(duration >= Duration::from_millis(10));
    }

    #[test]
    fn test_timed_with_result() {
        let (duration, result): (Duration, Result<i32, &str>) = timed(|| Ok(42));

        assert!(duration < Duration::from_millis(10));
        assert_eq!(result, Ok(42));
    }

    // ProfiledConnection tests require an actual rusqlite connection
    mod profiled_connection_tests {
        use super::*;

        fn create_test_db() -> rusqlite::Connection {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
                .unwrap();
            conn
        }

        #[test]
        fn test_execute_profiled() {
            let conn = create_test_db();
            let profiler = QueryProfiler::new(Duration::from_millis(100));
            let profiled = ProfiledConnection::new(&conn, &profiler);

            let result = profiled.execute_profiled(
                "INSERT INTO test (value) VALUES (?)",
                rusqlite::params!["hello"],
            );

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 1); // 1 row affected

            let stats = profiler.stats();
            assert_eq!(stats.total_queries, 1);
            assert_eq!(stats.total_rows, 1);
        }

        #[test]
        fn test_execute_profiled_error() {
            let conn = create_test_db();
            let profiler = QueryProfiler::new(Duration::from_millis(100));
            let profiled = ProfiledConnection::new(&conn, &profiler);

            // Invalid SQL
            let result = profiled.execute_profiled("INSERT INTO nonexistent (x) VALUES (1)", []);

            assert!(result.is_err());

            // Should still record the query (with 0 rows)
            let stats = profiler.stats();
            assert_eq!(stats.total_queries, 1);
            assert_eq!(stats.total_rows, 0);
        }

        #[test]
        fn test_execute_batch_profiled() {
            let conn = create_test_db();
            let profiler = QueryProfiler::new(Duration::from_millis(100));
            let profiled = ProfiledConnection::new(&conn, &profiler);

            let result = profiled.execute_batch_profiled(
                "INSERT INTO test (value) VALUES ('a'); INSERT INTO test (value) VALUES ('b');",
            );

            assert!(result.is_ok());

            let stats = profiler.stats();
            assert_eq!(stats.total_queries, 1);
            // Batch doesn't report rows
            assert_eq!(stats.total_rows, 0);
        }

        #[test]
        fn test_query_row_profiled() {
            let conn = create_test_db();
            conn.execute("INSERT INTO test (value) VALUES ('hello')", [])
                .unwrap();

            let profiler = QueryProfiler::new(Duration::from_millis(100));
            let profiled = ProfiledConnection::new(&conn, &profiler);

            let result: rusqlite::Result<String> =
                profiled.query_row_profiled("SELECT value FROM test WHERE id = 1", [], |row| {
                    row.get(0)
                });

            assert_eq!(result.unwrap(), "hello");

            let stats = profiler.stats();
            assert_eq!(stats.total_queries, 1);
            assert_eq!(stats.total_rows, 1);
        }

        #[test]
        fn test_query_row_profiled_not_found() {
            let conn = create_test_db();
            let profiler = QueryProfiler::new(Duration::from_millis(100));
            let profiled = ProfiledConnection::new(&conn, &profiler);

            let result: rusqlite::Result<String> =
                profiled.query_row_profiled("SELECT value FROM test WHERE id = 999", [], |row| {
                    row.get(0)
                });

            assert!(result.is_err());

            let stats = profiler.stats();
            assert_eq!(stats.total_queries, 1);
            // Error records 0 rows
            assert_eq!(stats.total_rows, 0);
        }

        #[test]
        fn test_inner_returns_connection() {
            let conn = create_test_db();
            let profiler = QueryProfiler::new(Duration::from_millis(100));
            let profiled = ProfiledConnection::new(&conn, &profiler);

            // Should be able to use inner() for unprofiled operations
            let count: i64 = profiled
                .inner()
                .query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0))
                .unwrap();

            assert_eq!(count, 0);

            // inner() calls are not profiled
            assert_eq!(profiler.stats().total_queries, 0);
        }
    }
}
