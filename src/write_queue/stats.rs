//! Write queue statistics.

/// Statistics for monitoring queue health.
#[derive(Debug, Clone)]
pub struct QueueStats {
    /// Number of writes currently queued.
    pub pending: u64,
    /// Total writes that have been processed.
    pub processed: u64,
    /// Total writes dropped due to overflow policy.
    pub dropped: u64,
    /// Total write errors (SQLite failures).
    pub errors: u64,
    /// Queue capacity.
    pub capacity: usize,
}

impl QueueStats {
    /// Calculate queue utilization as a percentage (0.0 - 100.0).
    pub fn utilization(&self) -> f64 {
        if self.capacity == 0 {
            return 0.0;
        }
        (self.pending as f64 / self.capacity as f64) * 100.0
    }

    /// Check if queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.pending as usize >= self.capacity
    }
}
