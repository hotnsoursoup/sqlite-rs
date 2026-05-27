//! Configuration types for the write queue.

use std::time::Duration;

/// Policy for handling queue overflow when capacity is reached.
///
/// Different policies suit different use cases:
///
/// | Policy | Latency | Data Loss | Best For |
/// |--------|---------|-----------|----------|
/// | `Block` | Unbounded | None | Batch jobs, non-interactive |
/// | `BlockTimeout` | Bounded | Possible | Most services (safe default) |
/// | `Reject` | Zero | Possible | Services with own retry logic |
/// | `DropOldest` | Zero | Yes | Metrics/telemetry |
#[derive(Debug, Clone)]
pub enum OverflowPolicy {
    /// Block indefinitely until space is available.
    ///
    /// - **Latency**: Unbounded
    /// - **Data loss**: None
    /// - **Use case**: Batch jobs, background tasks where latency doesn't matter
    ///
    /// # Warning
    ///
    /// Can cause cascading slowdowns if the writer can't keep up.
    /// Consider `BlockTimeout` for most production use cases.
    Block,

    /// Block up to the specified duration, then return `QueueFull` error.
    ///
    /// - **Latency**: Bounded by duration
    /// - **Data loss**: Possible (caller receives error)
    /// - **Use case**: Most production services (recommended default)
    ///
    /// This is the safest default - bounded latency with explicit feedback
    /// to the caller when the system is overloaded.
    BlockTimeout(Duration),

    /// Return `QueueFull` error immediately without waiting.
    ///
    /// - **Latency**: Zero
    /// - **Data loss**: Possible (caller receives error)
    /// - **Use case**: Services with their own retry/backoff logic
    ///
    /// Useful when the caller wants to implement custom backpressure handling.
    Reject,

    /// Drop the oldest queued item to make room for the new one.
    ///
    /// - **Latency**: Zero
    /// - **Data loss**: Yes (silent, oldest item dropped)
    /// - **Use case**: Metrics, telemetry, logging where latest > complete
    ///
    /// # Warning
    ///
    /// Only appropriate for fire-and-forget writes where losing old data
    /// is acceptable. The dropped write's handle (if any) will receive
    /// a `Dropped` error.
    DropOldest,
}

impl Default for OverflowPolicy {
    /// Default policy is `BlockTimeout(5 seconds)` - bounded latency with feedback.
    fn default() -> Self {
        OverflowPolicy::BlockTimeout(Duration::from_secs(5))
    }
}

/// Configuration for the write queue.
///
/// # Example
///
/// ```rust
/// use sqlite_rs::write_queue::{WriteQueueConfig, OverflowPolicy};
/// use std::time::Duration;
///
/// // Default config: 1000 capacity, 5s timeout
/// let config = WriteQueueConfig::default();
///
/// // High-throughput config for metrics
/// let metrics_config = WriteQueueConfig {
///     capacity: 10_000,
///     overflow_policy: OverflowPolicy::DropOldest,
/// };
///
/// // Strict config for critical writes
/// let critical_config = WriteQueueConfig {
///     capacity: 100,
///     overflow_policy: OverflowPolicy::BlockTimeout(Duration::from_secs(30)),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct WriteQueueConfig {
    /// Maximum number of pending writes in the queue.
    ///
    /// When this limit is reached, the `overflow_policy` determines behavior.
    ///
    /// Default: 1000
    pub capacity: usize,

    /// Policy when queue is full.
    ///
    /// Default: `BlockTimeout(5 seconds)`
    pub overflow_policy: OverflowPolicy,
}

impl Default for WriteQueueConfig {
    fn default() -> Self {
        Self {
            capacity: 1000,
            overflow_policy: OverflowPolicy::default(),
        }
    }
}

impl WriteQueueConfig {
    /// Create a new config with the specified capacity and default overflow policy.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is 0.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "write queue capacity must be greater than 0");
        Self {
            capacity,
            ..Default::default()
        }
    }

    /// Set the overflow policy.
    pub fn with_overflow_policy(mut self, policy: OverflowPolicy) -> Self {
        self.overflow_policy = policy;
        self
    }

    /// Create a config optimized for metrics/telemetry (high capacity, drop oldest).
    pub fn for_metrics() -> Self {
        Self {
            capacity: 10_000,
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }

    /// Create a config for critical writes (lower capacity, longer timeout).
    pub fn for_critical() -> Self {
        Self {
            capacity: 100,
            overflow_policy: OverflowPolicy::BlockTimeout(Duration::from_secs(30)),
        }
    }

    /// Create a config that never blocks (immediate reject on full).
    pub fn non_blocking() -> Self {
        Self {
            capacity: 1000,
            overflow_policy: OverflowPolicy::Reject,
        }
    }
}
