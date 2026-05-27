//! Database maintenance utilities.
//!
//! Provides connection health checks, vacuum operations, and other
//! database maintenance tasks.
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_rs::maintenance::{health_check, vacuum, analyze};
//!
//! # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
//! // Check database health
//! let health = health_check(pool).await?;
//! if health.is_healthy() {
//!     println!("Database is healthy");
//! }
//!
//! // Run vacuum to reclaim space
//! vacuum(pool).await?;
//!
//! // Update statistics for query optimizer
//! analyze(pool).await?;
//! # Ok(())
//! # }
//! ```

mod health;
mod vacuum;

pub use health::{health_check, ping, HealthCheck, HealthStatus};
pub use vacuum::{
    analyze, analyze_table, incremental_vacuum, optimize, set_auto_vacuum, vacuum, vacuum_into,
    VacuumStats,
};
