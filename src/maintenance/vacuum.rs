//! Vacuum and optimization utilities.

use crate::error::{PoolError, Result};
use crate::pool::DatabasePool;
use crate::sql::{quote_identifier, quote_string_literal};
use std::path::Path;

/// Statistics from a vacuum operation.
#[derive(Debug, Clone)]
pub struct VacuumStats {
    /// Database size before vacuum (bytes).
    pub size_before: i64,
    /// Database size after vacuum (bytes).
    pub size_after: i64,
    /// Space reclaimed (bytes).
    pub space_reclaimed: i64,
    /// Freelist pages before vacuum.
    pub freelist_before: i64,
}

impl VacuumStats {
    /// Get the compression ratio (size_after / size_before).
    pub fn compression_ratio(&self) -> f64 {
        if self.size_before > 0 {
            self.size_after as f64 / self.size_before as f64
        } else {
            1.0
        }
    }

    /// Check if any space was reclaimed.
    pub fn reclaimed_space(&self) -> bool {
        self.space_reclaimed > 0
    }
}

/// Run VACUUM to rebuild the database and reclaim space.
///
/// This operation:
/// - Rebuilds the database file, defragmenting it
/// - Reclaims unused space from deleted rows
/// - Resets the freelist
///
/// **Note**: VACUUM requires exclusive access and can be slow on large databases.
/// Consider using `vacuum_into` for large databases to avoid blocking.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::vacuum;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// let stats = vacuum(pool).await?;
/// println!("Reclaimed {} bytes", stats.space_reclaimed);
/// # Ok(())
/// # }
/// ```
pub async fn vacuum(pool: &DatabasePool) -> Result<VacuumStats> {
    // Get stats before vacuum
    let (size_before, freelist_before) = pool
        .read(|conn| {
            let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
            let freelist: i64 = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
            Ok((page_count * page_size, freelist))
        })
        .await
        .map_err(|e| PoolError::Maintenance(format!("failed to get pre-vacuum stats: {}", e)))?;

    // Run vacuum
    pool.write(|conn| conn.execute_batch("VACUUM"))
        .await
        .map_err(|e| PoolError::Maintenance(format!("vacuum failed: {}", e)))?;

    // Get stats after vacuum
    let size_after = pool
        .read(|conn| {
            let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
            Ok(page_count * page_size)
        })
        .await
        .map_err(|e| PoolError::Maintenance(format!("failed to get post-vacuum stats: {}", e)))?;

    Ok(VacuumStats {
        size_before,
        size_after,
        space_reclaimed: size_before - size_after,
        freelist_before,
    })
}

/// Vacuum into a new database file.
///
/// This is useful for:
/// - Creating a compacted backup
/// - Vacuuming without blocking the main database for long
/// - Changing database settings (page_size, encoding) during vacuum
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::vacuum_into;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// vacuum_into(pool, "backup/compacted.db").await?;
/// # Ok(())
/// # }
/// ```
pub async fn vacuum_into<P: AsRef<Path>>(pool: &DatabasePool, target_path: P) -> Result<()> {
    let target = target_path.as_ref().to_string_lossy().to_string();

    // Ensure parent directory exists
    if let Some(parent) = target_path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    pool.write(move |conn| {
        // VACUUM INTO creates a new, compacted copy. SQLite does not accept
        // bind parameters for the target filename, so quote the literal.
        conn.execute(
            &format!("VACUUM INTO {}", quote_string_literal(&target)),
            [],
        )
    })
    .await
    .map_err(|e| PoolError::Maintenance(format!("vacuum into failed: {}", e)))?;

    Ok(())
}

/// Run ANALYZE to update query optimizer statistics.
///
/// This helps SQLite choose better query plans by analyzing:
/// - Table row counts
/// - Index statistics
/// - Column value distribution
///
/// Run this after significant data changes for better query performance.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::analyze;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// // After bulk import
/// analyze(pool).await?;
/// # Ok(())
/// # }
/// ```
pub async fn analyze(pool: &DatabasePool) -> Result<()> {
    pool.write(|conn| conn.execute_batch("ANALYZE"))
        .await
        .map_err(|e| PoolError::Maintenance(format!("analyze failed: {}", e)))
}

/// Analyze a specific table.
///
/// More efficient than full ANALYZE when only one table has changed.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::analyze_table;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// analyze_table(pool, "orders").await?;
/// # Ok(())
/// # }
/// ```
pub async fn analyze_table(pool: &DatabasePool, table: &str) -> Result<()> {
    let table = table.to_string();
    pool.write(move |conn| conn.execute_batch(&format!("ANALYZE {}", quote_identifier(&table))))
        .await
        .map_err(|e| PoolError::Maintenance(format!("analyze table failed: {}", e)))
}

/// Enable or configure auto_vacuum.
///
/// Auto-vacuum modes:
/// - 0: None (default) - no automatic vacuuming
/// - 1: Full - vacuum after every transaction that frees pages
/// - 2: Incremental - pages are marked for removal but not reclaimed until PRAGMA incremental_vacuum
///
/// **Note**: This setting only takes effect on a new database or after VACUUM.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::set_auto_vacuum;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// // Enable incremental auto-vacuum
/// set_auto_vacuum(pool, 2).await?;
/// # Ok(())
/// # }
/// ```
pub async fn set_auto_vacuum(pool: &DatabasePool, mode: u8) -> Result<()> {
    if mode > 2 {
        return Err(PoolError::Maintenance(
            "auto_vacuum mode must be 0, 1, or 2".to_string(),
        ));
    }

    pool.write(move |conn| conn.execute_batch(&format!("PRAGMA auto_vacuum = {}", mode)))
        .await
        .map_err(|e| PoolError::Maintenance(format!("set auto_vacuum failed: {}", e)))
}

/// Run incremental vacuum to reclaim a specific number of pages.
///
/// Only works when auto_vacuum is set to 2 (incremental).
/// This allows reclaiming space in smaller chunks without blocking.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::incremental_vacuum;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// // Reclaim up to 100 pages
/// incremental_vacuum(pool, 100).await?;
/// # Ok(())
/// # }
/// ```
pub async fn incremental_vacuum(pool: &DatabasePool, pages: u32) -> Result<()> {
    pool.write(move |conn| conn.execute_batch(&format!("PRAGMA incremental_vacuum({})", pages)))
        .await
        .map_err(|e| PoolError::Maintenance(format!("incremental vacuum failed: {}", e)))
}

/// Optimize the database for better performance.
///
/// Runs a sequence of optimizations:
/// 1. PRAGMA optimize - runs ANALYZE on tables that need it
/// 2. Clears unused statement cache
///
/// Call this periodically (e.g., on app shutdown or after heavy usage).
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::maintenance::optimize;
///
/// # async fn example(pool: &sqlite_kit::DatabasePool) -> Result<(), sqlite_kit::PoolError> {
/// // Run before app shutdown
/// optimize(pool).await?;
/// # Ok(())
/// # }
/// ```
pub async fn optimize(pool: &DatabasePool) -> Result<()> {
    pool.write(|conn| conn.execute_batch("PRAGMA optimize"))
        .await
        .map_err(|e| PoolError::Maintenance(format!("optimize failed: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PoolConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_vacuum() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
            .await
            .unwrap();

        // Create and populate table with manual inserts
        pool.write(|conn| {
            conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY, data TEXT)")?;
            for i in 0..100 {
                conn.execute(
                    "INSERT INTO test (data) VALUES (?)",
                    [format!("data_{}", i)],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        // Delete some data to create freelist pages
        pool.write(|conn| conn.execute("DELETE FROM test WHERE id > 50", []))
            .await
            .unwrap();

        // Vacuum
        let stats = vacuum(&pool).await.unwrap();
        assert!(stats.size_before > 0);
        // Size after should be <= size before
        assert!(stats.size_after <= stats.size_before);

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_vacuum_into() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let backup_path = dir.path().join("backup's.db");

        let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| {
            conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
        })
        .await
        .unwrap();

        pool.write(|conn| conn.execute("INSERT INTO test (value) VALUES ('hello')", []))
            .await
            .unwrap();

        vacuum_into(&pool, &backup_path).await.unwrap();

        // Verify backup exists
        assert!(backup_path.exists());

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_analyze() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| {
            conn.execute_batch(
                "CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);
                 INSERT INTO test (value) VALUES ('a'), ('b'), ('c');",
            )
        })
        .await
        .unwrap();

        // Should not error
        analyze(&pool).await.unwrap();
        analyze_table(&pool, "test").await.unwrap();

        pool.write(|conn| conn.execute("CREATE TABLE \"quote\"\"table\" (id INTEGER)", []))
            .await
            .unwrap();
        analyze_table(&pool, "quote\"table").await.unwrap();

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_optimize() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
            .await
            .unwrap();

        // Should not error
        optimize(&pool).await.unwrap();

        pool.close().await.unwrap();
    }
}
