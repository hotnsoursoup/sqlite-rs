//! Database integrity checking utilities.

use crate::error::{PoolError, Result};
use crate::pool::DatabasePool;

/// An integrity issue found during checking.
#[derive(Debug, Clone)]
pub struct IntegrityIssue {
    /// Description of the issue.
    pub message: String,
}

/// A foreign key constraint violation.
#[derive(Debug, Clone)]
pub struct ForeignKeyViolation {
    /// Table containing the violation.
    pub table: String,
    /// Row ID of the violating row.
    pub rowid: i64,
    /// Referenced table.
    pub referenced_table: String,
    /// Foreign key constraint index.
    pub fk_index: i64,
}

/// Run a full integrity check on the database.
///
/// This checks:
/// - B-tree structure integrity
/// - Index consistency
/// - Page allocation
///
/// Returns an empty vec if the database is healthy.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::backup::integrity_check;
///
/// # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
/// let issues = integrity_check(pool).await?;
/// if issues.is_empty() {
///     println!("Database is healthy");
/// } else {
///     for issue in &issues {
///         eprintln!("Issue: {}", issue.message);
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub async fn integrity_check(pool: &DatabasePool) -> Result<Vec<IntegrityIssue>> {
    pool.read(|conn| {
        let mut stmt = conn.prepare("PRAGMA integrity_check")?;
        let rows: std::result::Result<Vec<_>, _> = stmt
            .query_map([], |row| {
                let message: String = row.get(0)?;
                Ok(IntegrityIssue { message })
            })?
            .collect();

        let issues = rows?
            .into_iter()
            .filter(|issue| issue.message != "ok")
            .collect();
        Ok(issues)
    })
    .await
    .map_err(|e| PoolError::Integrity(format!("integrity check failed: {}", e)))
}

/// Run a quick integrity check (faster but less thorough).
///
/// Only checks that the structure of each table and index is correct.
/// Does not verify that the content of tables is consistent with indexes.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::backup::quick_check;
///
/// # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
/// let issues = quick_check(pool).await?;
/// if issues.is_empty() {
///     println!("Quick check passed");
/// }
/// # Ok(())
/// # }
/// ```
pub async fn quick_check(pool: &DatabasePool) -> Result<Vec<IntegrityIssue>> {
    pool.read(|conn| {
        let mut stmt = conn.prepare("PRAGMA quick_check")?;
        let rows: std::result::Result<Vec<_>, _> = stmt
            .query_map([], |row| {
                let message: String = row.get(0)?;
                Ok(IntegrityIssue { message })
            })?
            .collect();

        let issues = rows?
            .into_iter()
            .filter(|issue| issue.message != "ok")
            .collect();
        Ok(issues)
    })
    .await
    .map_err(|e| PoolError::Integrity(format!("quick check failed: {}", e)))
}

/// Check for foreign key constraint violations.
///
/// Returns all rows that violate foreign key constraints.
/// Useful for validating data integrity after imports or migrations.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::backup::foreign_key_check;
///
/// # async fn example(pool: &sqlite_rs::DatabasePool) -> Result<(), sqlite_rs::PoolError> {
/// let violations = foreign_key_check(pool).await?;
/// for v in &violations {
///     eprintln!(
///         "FK violation: {} row {} references missing {} row",
///         v.table, v.rowid, v.referenced_table
///     );
/// }
/// # Ok(())
/// # }
/// ```
pub async fn foreign_key_check(pool: &DatabasePool) -> Result<Vec<ForeignKeyViolation>> {
    pool.read(|conn| {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let rows: std::result::Result<Vec<_>, _> = stmt
            .query_map([], |row| {
                Ok(ForeignKeyViolation {
                    table: row.get(0)?,
                    rowid: row.get(1)?,
                    referenced_table: row.get(2)?,
                    fk_index: row.get(3)?,
                })
            })?
            .collect();
        rows
    })
    .await
    .map_err(|e| PoolError::Integrity(format!("foreign key check failed: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_integrity_check_healthy() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, crate::PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
            .await
            .unwrap();

        let issues = integrity_check(&pool).await.unwrap();
        assert!(issues.is_empty());

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_quick_check() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, crate::PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
            .await
            .unwrap();

        let issues = quick_check(&pool).await.unwrap();
        assert!(issues.is_empty());

        pool.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_foreign_key_check() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = DatabasePool::open(&db_path, crate::PoolConfig::minimal())
            .await
            .unwrap();

        pool.write(|conn| {
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE parent (id INTEGER PRIMARY KEY);
                 CREATE TABLE child (
                     id INTEGER PRIMARY KEY,
                     parent_id INTEGER REFERENCES parent(id)
                 );
                 INSERT INTO parent (id) VALUES (1);
                 INSERT INTO child (id, parent_id) VALUES (1, 1);",
            )
        })
        .await
        .unwrap();

        // No violations initially
        let violations = foreign_key_check(&pool).await.unwrap();
        assert!(violations.is_empty());

        // Create violation by deleting parent (with FK off temporarily)
        pool.write(|conn| {
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DELETE FROM parent WHERE id = 1;",
            )
        })
        .await
        .unwrap();

        // Now we should see a violation
        let violations = foreign_key_check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].table, "child");

        pool.close().await.unwrap();
    }
}
