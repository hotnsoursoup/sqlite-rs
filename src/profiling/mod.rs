//! Query profiling and slow query logging.
//!
//! Provides tools for monitoring query performance, identifying slow queries,
//! and collecting execution statistics.
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_rs::profiling::{QueryStats, SlowQueryLogger};
//! use std::time::Duration;
//!
//! # fn example() {
//! // Create a slow query logger
//! let logger = SlowQueryLogger::new(Duration::from_millis(100));
//!
//! // Log slow queries
//! logger.log(&QueryStats {
//!     sql: "SELECT * FROM large_table".to_string(),
//!     duration: Duration::from_millis(250),
//!     rows_affected: 10000,
//! });
//! # }
//! ```

mod logger;
mod profiler;
mod stats;

pub use logger::{SlowQueryCallback, SlowQueryLogger};
pub use profiler::{timed, ProfiledConnection, QueryProfiler};
pub use stats::{AggregateStats, QueryStats};
