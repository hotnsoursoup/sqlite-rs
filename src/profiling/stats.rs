//! Query statistics types.

use std::time::Duration;

/// Maximum length for stored SQL queries.
pub const MAX_SQL_LENGTH: usize = 1000;

/// Statistics for a single query execution.
#[derive(Debug, Clone)]
pub struct QueryStats {
    /// The SQL query (may be truncated for very long queries).
    pub sql: String,
    /// How long the query took to execute.
    pub duration: Duration,
    /// Number of rows affected (for writes) or returned (for reads).
    pub rows_affected: u64,
}

impl QueryStats {
    /// Create new query stats.
    pub fn new(sql: impl Into<String>, duration: Duration, rows_affected: u64) -> Self {
        let sql = sql.into();
        // Truncate very long queries for storage (UTF-8 safe)
        let sql = truncate_sql(&sql, MAX_SQL_LENGTH);

        Self {
            sql,
            duration,
            rows_affected,
        }
    }

    /// Check if this query exceeds the given threshold.
    ///
    /// A query is considered slow if its duration is strictly greater than the threshold.
    pub fn is_slow(&self, threshold: Duration) -> bool {
        self.duration > threshold
    }
}

/// Truncate SQL string safely, respecting UTF-8 character boundaries.
///
/// Returns the original string if it's within the limit, otherwise truncates
/// at a valid character boundary and appends "...".
/// The resulting string is guaranteed to be <= max_len bytes.
fn truncate_sql(sql: &str, max_len: usize) -> String {
    if sql.len() <= max_len {
        return sql.to_string();
    }

    // We need room for "..." (3 bytes), so find boundary before max_len - 3
    let target_len = max_len.saturating_sub(3);

    // Find the last valid character boundary that keeps us under target_len
    let mut boundary = 0;
    for (i, c) in sql.char_indices() {
        let char_end = i + c.len_utf8();
        if char_end <= target_len {
            boundary = char_end;
        } else {
            break;
        }
    }

    format!("{}...", &sql[..boundary])
}

/// Aggregate statistics across multiple queries.
#[derive(Debug, Clone, Default)]
pub struct AggregateStats {
    /// Total number of queries executed.
    pub total_queries: u64,
    /// Total execution time across all queries (saturates at Duration::MAX).
    pub total_duration: Duration,
    /// Number of slow queries (above threshold).
    pub slow_queries: u64,
    /// Longest query duration seen.
    pub max_duration: Duration,
    /// The slowest query's SQL.
    pub slowest_query: Option<String>,
    /// Total rows affected across all queries (saturates at u64::MAX).
    pub total_rows: u64,
}

impl AggregateStats {
    /// Create a new empty stats collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a query's statistics.
    ///
    /// Uses saturating arithmetic to prevent overflow panics in long-running systems.
    pub fn record(&mut self, stats: &QueryStats, slow_threshold: Duration) {
        self.total_queries = self.total_queries.saturating_add(1);
        self.total_duration = self.total_duration.saturating_add(stats.duration);
        self.total_rows = self.total_rows.saturating_add(stats.rows_affected);

        if stats.is_slow(slow_threshold) {
            self.slow_queries = self.slow_queries.saturating_add(1);
        }

        if stats.duration > self.max_duration {
            self.max_duration = stats.duration;
            self.slowest_query = Some(stats.sql.clone());
        }
    }

    /// Get the average query duration.
    ///
    /// Returns `Duration::ZERO` if no queries have been recorded.
    /// Uses 128-bit arithmetic internally to handle large query counts without overflow.
    pub fn avg_duration(&self) -> Duration {
        if self.total_queries == 0 {
            Duration::ZERO
        } else {
            // Use u128 to avoid overflow with large query counts
            let total_nanos = self.total_duration.as_nanos();
            let avg_nanos = total_nanos / self.total_queries as u128;
            // Clamp to u64::MAX nanos (~584 years) - should never happen in practice
            Duration::from_nanos(avg_nanos.min(u64::MAX as u128) as u64)
        }
    }

    /// Get the percentage of queries that were slow.
    pub fn slow_percentage(&self) -> f64 {
        if self.total_queries == 0 {
            0.0
        } else {
            (self.slow_queries as f64 / self.total_queries as f64) * 100.0
        }
    }

    /// Merge another stats object into this one.
    ///
    /// Uses saturating arithmetic to prevent overflow.
    pub fn merge(&mut self, other: &AggregateStats) {
        self.total_queries = self.total_queries.saturating_add(other.total_queries);
        self.total_duration = self.total_duration.saturating_add(other.total_duration);
        self.slow_queries = self.slow_queries.saturating_add(other.slow_queries);
        self.total_rows = self.total_rows.saturating_add(other.total_rows);

        if other.max_duration > self.max_duration {
            self.max_duration = other.max_duration;
            self.slowest_query = other.slowest_query.clone();
        }
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_stats_truncation_ascii() {
        let long_sql = "x".repeat(2000);
        let stats = QueryStats::new(long_sql, Duration::from_millis(100), 1);
        assert!(stats.sql.len() <= MAX_SQL_LENGTH);
        assert!(stats.sql.ends_with("..."));
    }

    #[test]
    fn test_query_stats_truncation_utf8() {
        // Test with multi-byte characters (emoji is 4 bytes)
        let emoji_sql = "🔥".repeat(500); // 2000 bytes
        let stats = QueryStats::new(emoji_sql, Duration::from_millis(100), 1);

        // Should not panic and should be valid UTF-8
        assert!(stats.sql.len() <= MAX_SQL_LENGTH);
        assert!(stats.sql.ends_with("..."));
        // Verify it's valid UTF-8 by iterating chars
        assert!(stats.sql.chars().count() > 0);
    }

    #[test]
    fn test_query_stats_truncation_mixed_utf8() {
        // Mix of ASCII and multi-byte chars
        let mixed = format!("SELECT * FROM {} WHERE id = 1", "テーブル".repeat(200));
        let stats = QueryStats::new(mixed, Duration::from_millis(100), 1);

        assert!(stats.sql.len() <= MAX_SQL_LENGTH);
        assert!(stats.sql.ends_with("..."));
        // Verify valid UTF-8
        for c in stats.sql.chars() {
            assert!(c.len_utf8() > 0);
        }
    }

    #[test]
    fn test_query_stats_no_truncation_short() {
        let short_sql = "SELECT 1";
        let stats = QueryStats::new(short_sql, Duration::from_millis(10), 1);
        assert_eq!(stats.sql, "SELECT 1");
    }

    #[test]
    fn test_aggregate_stats() {
        let mut agg = AggregateStats::new();
        let threshold = Duration::from_millis(100);

        // Fast query
        agg.record(
            &QueryStats::new("SELECT 1", Duration::from_millis(10), 1),
            threshold,
        );

        // Slow query
        agg.record(
            &QueryStats::new("SELECT * FROM big_table", Duration::from_millis(500), 1000),
            threshold,
        );

        assert_eq!(agg.total_queries, 2);
        assert_eq!(agg.slow_queries, 1);
        assert_eq!(agg.max_duration, Duration::from_millis(500));
        assert_eq!(
            agg.slowest_query,
            Some("SELECT * FROM big_table".to_string())
        );
        assert_eq!(agg.slow_percentage(), 50.0);
    }

    #[test]
    fn test_avg_duration_empty() {
        let agg = AggregateStats::new();
        assert_eq!(agg.avg_duration(), Duration::ZERO);
    }

    #[test]
    fn test_avg_duration_large_count() {
        let mut agg = AggregateStats::new();
        // Simulate many queries by directly setting fields
        agg.total_queries = u64::MAX;
        agg.total_duration = Duration::from_secs(1_000_000);

        // Should not panic - uses u128 internally
        let avg = agg.avg_duration();
        assert!(avg < Duration::from_secs(1)); // Very small average
    }

    #[test]
    fn test_saturating_arithmetic() {
        let mut agg = AggregateStats::new();
        agg.total_queries = u64::MAX;
        agg.total_rows = u64::MAX;

        // Should saturate, not panic
        agg.record(
            &QueryStats::new("SELECT 1", Duration::from_millis(10), 100),
            Duration::from_millis(100),
        );

        assert_eq!(agg.total_queries, u64::MAX);
        assert_eq!(agg.total_rows, u64::MAX);
    }

    #[test]
    fn test_merge() {
        let mut agg1 = AggregateStats::new();
        let mut agg2 = AggregateStats::new();
        let threshold = Duration::from_millis(100);

        agg1.record(
            &QueryStats::new("SELECT 1", Duration::from_millis(10), 5),
            threshold,
        );
        agg2.record(
            &QueryStats::new("SELECT 2", Duration::from_millis(200), 10),
            threshold,
        );

        agg1.merge(&agg2);

        assert_eq!(agg1.total_queries, 2);
        assert_eq!(agg1.total_rows, 15);
        assert_eq!(agg1.slow_queries, 1);
        assert_eq!(agg1.max_duration, Duration::from_millis(200));
    }

    #[test]
    fn test_reset() {
        let mut agg = AggregateStats::new();
        agg.record(
            &QueryStats::new("SELECT 1", Duration::from_millis(10), 1),
            Duration::from_millis(100),
        );

        agg.reset();

        assert_eq!(agg.total_queries, 0);
        assert_eq!(agg.total_rows, 0);
        assert_eq!(agg.total_duration, Duration::ZERO);
        assert!(agg.slowest_query.is_none());
    }
}
