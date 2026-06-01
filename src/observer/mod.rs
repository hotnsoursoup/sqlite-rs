//! Operation observer system for logging, metrics, profiling, and policy hooks.
//!
//! Observers are attached directly to [`PoolConfig`](crate::PoolConfig) and run
//! from [`DatabasePool`](crate::DatabasePool). This keeps cross-cutting concerns
//! in one place: query profiling, logging, metrics, and blocking policy all use
//! the same [`Observer`] lifecycle.
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_kit::{DatabasePool, PoolConfig};
//! use sqlite_kit::observer::{MetricsCollector, QueryLogger};
//! use sqlite_kit::profiling::QueryProfiler;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), sqlite_kit::PoolError> {
//! let metrics = MetricsCollector::new();
//! let profiler = QueryProfiler::new(Duration::from_millis(100));
//!
//! let config = PoolConfig::default()
//!     .with_observer(QueryLogger::new())
//!     .with_observer(metrics.clone())
//!     .with_observer(profiler.clone());
//!
//! let pool = DatabasePool::open("data.db", config).await?;
//! pool.read(|conn| conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))).await?;
//!
//! assert_eq!(metrics.snapshot().total_queries, 1);
//! assert_eq!(profiler.stats().total_queries, 1);
//! # Ok(())
//! # }
//! ```

mod context;
mod logging;
mod metrics;
mod stack;
mod traits;

pub use context::{OperationContext, OperationKind};
pub use logging::QueryLogger;
pub use metrics::{MetricsCollector, QueryMetrics};
pub use stack::ObserverStack;
pub use traits::{Observer, ObserverError, ObserverResult};
