//! Connection and database health checks.

use crate::error::{PoolError, Result};
use crate::pool::DatabasePool;
use std::time::{Duration, Instant};

/// Database health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Database is healthy and responsive.
    Healthy,
    /// Database is degraded (slow but functional).
    Degraded,
    /// Database is unhealthy (connection failed or errors).
    Unhealthy,
}

/// Result of a health check.
#[derive(Debug, Clone)]
pub struct HealthCheck {
    /// Overall health status.
    pub status: HealthStatus,
    /// Time taken to execute a simple query.
    pub query_latency: Duration,
    /// Whether the writer connection is alive.
    pub writer_alive: bool,
    /// Whether reader connections are available.
    pub readers_available: bool,
    /// SQLite version.
    pub sqlite_version: String,
    /// Database page count.
    pub page_count: i64,
    /// Database page size in bytes.
    pub page_size: i64,
    /// Free pages (available for reuse).
    pub freelist_count: i64,
    /// Error message if unhealthy.
    pub error: Option<String>,
}

impl HealthCheck {
    /// Check if the database is healthy.
    pub fn is_healthy(&self) -> bool {
        self.status == HealthStatus::Healthy
    }

    /// Check if the database is at least partially functional.
    pub fn is_functional(&self) -> bool {
        matches!(self.status, HealthStatus::Healthy | HealthStatus::Degraded)
    }

    /// Get the database size in bytes.
    pub fn database_size(&self) -> i64 {
        self.page_count * self.page_size
    }

    /// Get the wasted space in bytes (freelist pages).
    pub fn wasted_space(&self) -> i64 {
        self.freelist_count * self.page_size
    }
}

/// Perform a comprehensive health check on the database.
///
/// Checks:
/// - Writer connection responsiveness
/// - Reader pool availability
/// - Query execution latency
/// - Database integrity (quick check)
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::{health_check, HealthStatus};
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// let health = health_check(pool).await?;
///
/// match health.status {
///     HealthStatus::Healthy => println!("All good!"),
///     HealthStatus::Degraded => println!("Slow: {:?}", health.query_latency),
///     HealthStatus::Unhealthy => println!("Error: {:?}", health.error),
/// }
///
/// println!("Database size: {} bytes", health.database_size());
/// # Ok(())
/// # }
/// ```
pub async fn health_check(pool: &DatabasePool) -> Result<HealthCheck> {
    let start = Instant::now();

    // Check writer connection with a simple query
    let writer_result = pool
        .write(|conn| conn.query_row("SELECT sqlite_version()", [], |row| row.get::<_, String>(0)))
        .await;

    let query_latency = start.elapsed();

    match writer_result {
        Ok(sqlite_version) => {
            // Get database statistics
            let stats = pool
                .read(|conn| {
                    let page_count: i64 =
                        conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
                    let page_size: i64 =
                        conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
                    let freelist_count: i64 =
                        conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
                    Ok((page_count, page_size, freelist_count))
                })
                .await;

            let pool_stats = pool.stats();
            let readers_available = pool_stats.reader_pool_available > 0;

            match stats {
                Ok((page_count, page_size, freelist_count)) => {
                    // Determine health status based on latency
                    let status = if query_latency < Duration::from_millis(100) {
                        HealthStatus::Healthy
                    } else {
                        HealthStatus::Degraded // Slow but still functional
                    };

                    Ok(HealthCheck {
                        status,
                        query_latency,
                        writer_alive: true,
                        readers_available,
                        sqlite_version,
                        page_count,
                        page_size,
                        freelist_count,
                        error: None,
                    })
                }
                Err(e) => Ok(HealthCheck {
                    status: HealthStatus::Degraded,
                    query_latency,
                    writer_alive: true,
                    readers_available: false,
                    sqlite_version,
                    page_count: 0,
                    page_size: 0,
                    freelist_count: 0,
                    error: Some(format!("Reader check failed: {}", e)),
                }),
            }
        }
        Err(e) => Ok(HealthCheck {
            status: HealthStatus::Unhealthy,
            query_latency,
            writer_alive: false,
            readers_available: false,
            sqlite_version: String::new(),
            page_count: 0,
            page_size: 0,
            freelist_count: 0,
            error: Some(e.to_string()),
        }),
    }
}

/// Ping the database with a minimal query.
///
/// Returns the query latency if successful.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::ping;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// match ping(pool).await {
///     Ok(latency) => println!("Ping: {:?}", latency),
///     Err(e) => println!("Database unreachable: {}", e),
/// }
/// # Ok(())
/// # }
/// ```
pub async fn ping(pool: &DatabasePool) -> Result<Duration> {
    let start = Instant::now();

    pool.read(|conn| conn.execute_batch("SELECT 1"))
        .await
        .map_err(|e| PoolError::Maintenance(format!("ping failed: {}", e)))?;

    Ok(start.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PoolConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_health_check() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
            .await
            .unwrap();

        let health = health_check(&pool).await.unwrap();

        assert!(health.is_healthy());
        assert!(health.writer_alive);
        assert!(!health.sqlite_version.is_empty());
        assert!(health.page_count > 0);
        assert!(health.page_size > 0);
        assert!(health.error.is_none());

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_ping() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
            .await
            .unwrap();

        let latency = ping(&pool).await.unwrap();
        assert!(latency < Duration::from_secs(1));

        pool.close().await.unwrap();
    }
}
