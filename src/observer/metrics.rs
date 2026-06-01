//! Metrics collection observer.

use super::context::{OperationContext, OperationKind};
use super::traits::Observer;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Collected query metrics.
#[derive(Debug, Clone, Default)]
pub struct QueryMetrics {
    /// Total number of queries executed.
    pub total_queries: u64,
    /// Number of successful queries.
    pub successful_queries: u64,
    /// Number of failed queries.
    pub failed_queries: u64,
    /// Number of slow queries.
    pub slow_queries: u64,
    /// Total execution time in milliseconds.
    pub total_time_ms: u64,
    /// Number of SELECT queries.
    pub select_count: u64,
    /// Number of INSERT queries.
    pub insert_count: u64,
    /// Number of UPDATE queries.
    pub update_count: u64,
    /// Number of DELETE queries.
    pub delete_count: u64,
    /// Number of other queries.
    pub other_count: u64,
}

impl QueryMetrics {
    /// Calculate the average latency in milliseconds.
    ///
    /// Returns 0.0 if no queries have been executed.
    pub fn avg_latency_ms(&self) -> f64 {
        if self.total_queries == 0 {
            0.0
        } else {
            self.total_time_ms as f64 / self.total_queries as f64
        }
    }

    /// Calculate the success rate as a percentage (0.0 - 100.0).
    ///
    /// Returns 100.0 if no queries have been executed.
    pub fn success_rate(&self) -> f64 {
        if self.total_queries == 0 {
            100.0
        } else {
            (self.successful_queries as f64 / self.total_queries as f64) * 100.0
        }
    }

    /// Calculate the slow query rate as a percentage (0.0 - 100.0).
    ///
    /// Returns 0.0 if no queries have been executed.
    pub fn slow_query_rate(&self) -> f64 {
        if self.total_queries == 0 {
            0.0
        } else {
            (self.slow_queries as f64 / self.total_queries as f64) * 100.0
        }
    }
}

/// Internal atomic counters for thread-safe metrics.
struct AtomicMetrics {
    total_queries: AtomicU64,
    successful_queries: AtomicU64,
    failed_queries: AtomicU64,
    slow_queries: AtomicU64,
    total_time_ms: AtomicU64,
    select_count: AtomicU64,
    insert_count: AtomicU64,
    update_count: AtomicU64,
    delete_count: AtomicU64,
    other_count: AtomicU64,
}

impl Default for AtomicMetrics {
    fn default() -> Self {
        Self {
            total_queries: AtomicU64::new(0),
            successful_queries: AtomicU64::new(0),
            failed_queries: AtomicU64::new(0),
            slow_queries: AtomicU64::new(0),
            total_time_ms: AtomicU64::new(0),
            select_count: AtomicU64::new(0),
            insert_count: AtomicU64::new(0),
            update_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
            other_count: AtomicU64::new(0),
        }
    }
}

impl AtomicMetrics {
    fn snapshot(&self) -> QueryMetrics {
        QueryMetrics {
            total_queries: self.total_queries.load(Ordering::Relaxed),
            successful_queries: self.successful_queries.load(Ordering::Relaxed),
            failed_queries: self.failed_queries.load(Ordering::Relaxed),
            slow_queries: self.slow_queries.load(Ordering::Relaxed),
            total_time_ms: self.total_time_ms.load(Ordering::Relaxed),
            select_count: self.select_count.load(Ordering::Relaxed),
            insert_count: self.insert_count.load(Ordering::Relaxed),
            update_count: self.update_count.load(Ordering::Relaxed),
            delete_count: self.delete_count.load(Ordering::Relaxed),
            other_count: self.other_count.load(Ordering::Relaxed),
        }
    }

    fn reset(&self) {
        self.total_queries.store(0, Ordering::Relaxed);
        self.successful_queries.store(0, Ordering::Relaxed);
        self.failed_queries.store(0, Ordering::Relaxed);
        self.slow_queries.store(0, Ordering::Relaxed);
        self.total_time_ms.store(0, Ordering::Relaxed);
        self.select_count.store(0, Ordering::Relaxed);
        self.insert_count.store(0, Ordering::Relaxed);
        self.update_count.store(0, Ordering::Relaxed);
        self.delete_count.store(0, Ordering::Relaxed);
        self.other_count.store(0, Ordering::Relaxed);
    }
}

/// Observer for collecting query metrics.
///
/// Collects statistics about query execution including:
/// - Total query count
/// - Success/failure counts
/// - Slow query count
/// - Average latency
/// - Counts by operation type (SELECT, INSERT, etc.)
///
/// # Thread Safety
///
/// Uses atomic operations for lock-free metric updates.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::observer::MetricsCollector;
///
/// let collector = MetricsCollector::new();
///
/// // After attaching to PoolConfig with `with_observer`...
/// let metrics = collector.snapshot();
/// println!("Total queries: {}", metrics.total_queries);
/// println!("Success rate: {:.1}%", metrics.success_rate());
/// ```
pub struct MetricsCollector {
    /// Slow query threshold in milliseconds.
    slow_threshold_ms: u64,
    /// Atomic metrics storage (atomics provide thread-safety, no lock needed).
    metrics: Arc<AtomicMetrics>,
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self {
            slow_threshold_ms: 100,
            metrics: Arc::new(AtomicMetrics::default()),
        }
    }
}

impl Clone for MetricsCollector {
    fn clone(&self) -> Self {
        Self {
            slow_threshold_ms: self.slow_threshold_ms,
            metrics: self.metrics.clone(),
        }
    }
}

impl MetricsCollector {
    /// Create a new metrics collector with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the slow query threshold in milliseconds.
    pub fn with_slow_threshold_ms(mut self, threshold: u64) -> Self {
        self.slow_threshold_ms = threshold;
        self
    }

    /// Get a snapshot of current metrics.
    pub fn snapshot(&self) -> QueryMetrics {
        self.metrics.snapshot()
    }

    /// Reset all metrics to zero.
    pub fn reset(&self) {
        self.metrics.reset();
    }
}

impl Observer for MetricsCollector {
    fn after_operation(&self, ctx: &OperationContext, duration: Duration, success: bool) {
        let duration_ms = duration.as_millis() as u64;
        // Update total and time
        self.metrics.total_queries.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .total_time_ms
            .fetch_add(duration_ms, Ordering::Relaxed);

        // Update success/failure
        if success {
            self.metrics
                .successful_queries
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics.failed_queries.fetch_add(1, Ordering::Relaxed);
        }

        // Update slow query count
        if duration_ms >= self.slow_threshold_ms {
            self.metrics.slow_queries.fetch_add(1, Ordering::Relaxed);
        }

        // Update operation type counts
        match ctx.operation() {
            OperationKind::Select => {
                self.metrics.select_count.fetch_add(1, Ordering::Relaxed);
            }
            OperationKind::Insert => {
                self.metrics.insert_count.fetch_add(1, Ordering::Relaxed);
            }
            OperationKind::Update => {
                self.metrics.update_count.fetch_add(1, Ordering::Relaxed);
            }
            OperationKind::Delete => {
                self.metrics.delete_count.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.metrics.other_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn name(&self) -> &'static str {
        "MetricsCollector"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_defaults() {
        let metrics = QueryMetrics::default();
        assert_eq!(metrics.total_queries, 0);
        assert_eq!(metrics.avg_latency_ms(), 0.0);
        assert_eq!(metrics.success_rate(), 100.0);
        assert_eq!(metrics.slow_query_rate(), 0.0);
    }

    #[test]
    fn test_metrics_calculations() {
        let metrics = QueryMetrics {
            total_queries: 100,
            successful_queries: 95,
            failed_queries: 5,
            slow_queries: 10,
            total_time_ms: 5000,
            ..Default::default()
        };

        assert_eq!(metrics.avg_latency_ms(), 50.0);
        assert_eq!(metrics.success_rate(), 95.0);
        assert_eq!(metrics.slow_query_rate(), 10.0);
    }

    #[test]
    fn test_collector_new() {
        let collector = MetricsCollector::new();
        let metrics = collector.snapshot();
        assert_eq!(metrics.total_queries, 0);
    }

    #[test]
    fn test_collector_after_operation() {
        let collector = MetricsCollector::new().with_slow_threshold_ms(50);

        // Successful fast query
        let ctx = OperationContext::new("SELECT 1");
        collector.after_operation(&ctx, Duration::from_millis(10), true);

        // Successful slow query
        collector.after_operation(&ctx, Duration::from_millis(100), true);

        // Failed query
        collector.after_operation(&ctx, Duration::from_millis(20), false);

        let metrics = collector.snapshot();
        assert_eq!(metrics.total_queries, 3);
        assert_eq!(metrics.successful_queries, 2);
        assert_eq!(metrics.failed_queries, 1);
        assert_eq!(metrics.slow_queries, 1);
        assert_eq!(metrics.select_count, 3);
        assert_eq!(metrics.total_time_ms, 130);
    }

    #[test]
    fn test_collector_reset() {
        let collector = MetricsCollector::new();

        let ctx = OperationContext::new("SELECT 1");
        collector.after_operation(&ctx, Duration::from_millis(10), true);

        assert_eq!(collector.snapshot().total_queries, 1);

        collector.reset();
        assert_eq!(collector.snapshot().total_queries, 0);
    }

    #[test]
    fn test_collector_operation_counts() {
        let collector = MetricsCollector::new();

        collector.after_operation(
            &OperationContext::new("SELECT 1"),
            Duration::from_millis(10),
            true,
        );
        collector.after_operation(
            &OperationContext::new("INSERT INTO t VALUES (1)"),
            Duration::from_millis(10),
            true,
        );
        collector.after_operation(
            &OperationContext::new("UPDATE t SET x = 1"),
            Duration::from_millis(10),
            true,
        );
        collector.after_operation(
            &OperationContext::new("DELETE FROM t"),
            Duration::from_millis(10),
            true,
        );
        collector.after_operation(
            &OperationContext::new("PRAGMA table_info(t)"),
            Duration::from_millis(10),
            true,
        );

        let metrics = collector.snapshot();
        assert_eq!(metrics.select_count, 1);
        assert_eq!(metrics.insert_count, 1);
        assert_eq!(metrics.update_count, 1);
        assert_eq!(metrics.delete_count, 1);
        assert_eq!(metrics.other_count, 1);
    }

    #[test]
    fn test_collector_clone_shares_metrics() {
        let collector1 = MetricsCollector::new();
        let collector2 = collector1.clone();

        let ctx = OperationContext::new("SELECT 1");
        collector1.after_operation(&ctx, Duration::from_millis(10), true);

        // Both should see the same metrics
        assert_eq!(collector1.snapshot().total_queries, 1);
        assert_eq!(collector2.snapshot().total_queries, 1);
    }

    #[test]
    fn test_collector_name() {
        let collector = MetricsCollector::new();
        assert_eq!(collector.name(), "MetricsCollector");
    }
}
