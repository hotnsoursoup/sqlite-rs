//! Retry configuration.

use std::time::Duration;

/// Strategy for calculating retry delays.
#[derive(Debug, Clone, Copy)]
pub enum RetryStrategy {
    /// Fixed delay between retries.
    Fixed(Duration),

    /// Exponential backoff: base_delay * 2^attempt.
    Exponential {
        /// Initial delay before first retry.
        base_delay: Duration,
        /// Maximum delay between retries.
        max_delay: Duration,
    },

    /// Linear backoff: base_delay * attempt.
    Linear {
        /// Delay increment per attempt.
        increment: Duration,
        /// Maximum delay between retries.
        max_delay: Duration,
    },
}

impl RetryStrategy {
    /// Calculate the delay for the given attempt number (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        match self {
            RetryStrategy::Fixed(d) => *d,
            RetryStrategy::Exponential {
                base_delay,
                max_delay,
            } => {
                let multiplier = 2u32.saturating_pow(attempt);
                let delay = base_delay.saturating_mul(multiplier);
                delay.min(*max_delay)
            }
            RetryStrategy::Linear {
                increment,
                max_delay,
            } => {
                let delay = increment.saturating_mul(attempt + 1);
                delay.min(*max_delay)
            }
        }
    }
}

impl Default for RetryStrategy {
    fn default() -> Self {
        Self::Exponential {
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
        }
    }
}

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_attempts: u32,

    /// Strategy for calculating delays between retries.
    pub strategy: RetryStrategy,

    /// Whether to add jitter to delays to avoid thundering herd.
    pub jitter: bool,

    /// Maximum total time to spend retrying (0 = unlimited).
    pub total_timeout: Duration,
}

impl RetryConfig {
    /// Create a new retry config with the given max attempts.
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            ..Default::default()
        }
    }

    /// Set the retry strategy.
    pub fn with_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Enable or disable jitter.
    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }

    /// Set the total timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.total_timeout = timeout;
        self
    }

    /// Create a config optimized for SQLITE_BUSY scenarios.
    ///
    /// Uses exponential backoff with moderate delays.
    pub fn for_busy() -> Self {
        Self {
            max_attempts: 5,
            strategy: RetryStrategy::Exponential {
                base_delay: Duration::from_millis(50),
                max_delay: Duration::from_secs(2),
            },
            jitter: true,
            total_timeout: Duration::from_secs(10),
        }
    }

    /// Create an aggressive retry config (many fast retries).
    pub fn aggressive() -> Self {
        Self {
            max_attempts: 10,
            strategy: RetryStrategy::Fixed(Duration::from_millis(5)),
            jitter: false,
            total_timeout: Duration::from_secs(5),
        }
    }

    /// Create a gentle retry config (few retries with longer delays).
    pub fn gentle() -> Self {
        Self {
            max_attempts: 3,
            strategy: RetryStrategy::Exponential {
                base_delay: Duration::from_millis(100),
                max_delay: Duration::from_secs(5),
            },
            jitter: true,
            total_timeout: Duration::from_secs(30),
        }
    }

    /// Calculate the delay for the given attempt, optionally with jitter.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_delay = self.strategy.delay_for_attempt(attempt);

        if self.jitter {
            // Add up to 25% random jitter to avoid thundering herd
            let jitter_range = base_delay.as_millis() as u64 / 4;
            if jitter_range > 0 {
                let jitter = fastrand::u64(0..jitter_range);
                base_delay + Duration::from_millis(jitter)
            } else {
                base_delay
            }
        } else {
            base_delay
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            strategy: RetryStrategy::default(),
            jitter: true,
            total_timeout: Duration::ZERO, // No timeout
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let strategy = RetryStrategy::Exponential {
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
        };

        assert_eq!(strategy.delay_for_attempt(0), Duration::from_millis(10));
        assert_eq!(strategy.delay_for_attempt(1), Duration::from_millis(20));
        assert_eq!(strategy.delay_for_attempt(2), Duration::from_millis(40));
        assert_eq!(strategy.delay_for_attempt(3), Duration::from_millis(80));
        // Should cap at max
        assert_eq!(strategy.delay_for_attempt(4), Duration::from_millis(100));
    }

    #[test]
    fn test_linear_backoff() {
        let strategy = RetryStrategy::Linear {
            increment: Duration::from_millis(50),
            max_delay: Duration::from_millis(200),
        };

        assert_eq!(strategy.delay_for_attempt(0), Duration::from_millis(50));
        assert_eq!(strategy.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(strategy.delay_for_attempt(2), Duration::from_millis(150));
        assert_eq!(strategy.delay_for_attempt(3), Duration::from_millis(200));
        // Should cap at max
        assert_eq!(strategy.delay_for_attempt(4), Duration::from_millis(200));
    }

    #[test]
    fn test_fixed_delay() {
        let strategy = RetryStrategy::Fixed(Duration::from_millis(100));

        assert_eq!(strategy.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(strategy.delay_for_attempt(5), Duration::from_millis(100));
    }
}
