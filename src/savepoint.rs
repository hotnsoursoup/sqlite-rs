//! Savepoint management for nested transaction control.
//!
//! Savepoints allow partial rollback within a transaction, enabling
//! fine-grained control over which operations to commit or undo.
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlite_kit::{DatabasePool, Savepoint};
//!
//! // Within a transaction closure:
//! pool.transaction(|tx| {
//!     // Insert base data
//!     tx.execute("INSERT INTO users (name) VALUES (?)", ["Alice"])?;
//!
//!     // Create a savepoint for risky operation
//!     let sp = Savepoint::new(tx, "risky_op")
//!         .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
//!
//!     // Try something that might fail
//!     match tx.execute("INSERT INTO audit (action) VALUES (?)", ["created"]) {
//!         Ok(_) => sp.commit()
//!             .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
//!         Err(_) => sp.rollback()
//!             .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
//!     }
//!
//!     Ok(())
//! }).await?;
//! ```

use crate::error::{PoolError, Result};
use crate::sql::quote_identifier;
use rusqlite::Transaction;
use std::cell::Cell;

/// A savepoint within a transaction.
///
/// Savepoints allow partial rollback within a transaction. When dropped
/// without explicit commit or rollback, a warning is logged.
///
/// # Lifecycle
///
/// - Create with [`Savepoint::new`]
/// - Execute queries via [`transaction()`](Savepoint::transaction)
/// - Finalize with [`commit()`](Savepoint::commit) or [`rollback()`](Savepoint::rollback)
///
/// # SQLite Syntax
///
/// - SAVEPOINT: `SAVEPOINT "name"`
/// - RELEASE: `RELEASE "name"`
/// - ROLLBACK TO: `ROLLBACK TO "name"`
pub struct Savepoint<'tx> {
    /// Reference to the parent transaction.
    tx: &'tx Transaction<'tx>,
    /// Savepoint name (validated on creation).
    name: String,
    /// Whether commit or rollback has been called.
    finalized: Cell<bool>,
}

impl std::fmt::Debug for Savepoint<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Savepoint")
            .field("name", &self.name)
            .field("finalized", &self.finalized.get())
            .finish_non_exhaustive()
    }
}

impl<'tx> Savepoint<'tx> {
    /// Create a new savepoint within a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - The parent transaction
    /// * `name` - Savepoint name (must be alphanumeric + underscores, not start with digit)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The name is invalid (empty, starts with digit, contains invalid characters)
    /// - The SAVEPOINT SQL command fails
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use sqlite_kit::Savepoint;
    /// # fn example(tx: &rusqlite::Transaction) -> Result<(), sqlite_kit::PoolError> {
    /// let sp = Savepoint::new(tx, "my_savepoint")?;
    /// // ... do work ...
    /// sp.commit()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(tx: &'tx Transaction<'tx>, name: impl Into<String>) -> Result<Self> {
        let name = name.into();

        if !is_valid_savepoint_name(&name) {
            return Err(PoolError::Savepoint(format!(
                "invalid savepoint name '{}': must be non-empty, alphanumeric + underscores, not start with digit",
                name
            )));
        }

        // Execute SAVEPOINT command
        let sql = format!("SAVEPOINT {}", quote_identifier(&name));
        tx.execute_batch(&sql).map_err(|e| {
            PoolError::Savepoint(format!("failed to create savepoint '{}': {}", name, e))
        })?;

        Ok(Self {
            tx,
            name,
            finalized: Cell::new(false),
        })
    }

    /// Commit the savepoint (RELEASE).
    ///
    /// This releases the savepoint, making all changes since the savepoint permanent
    /// within the transaction. The savepoint cannot be used after this.
    ///
    /// # Errors
    ///
    /// Returns an error if the RELEASE SQL command fails.
    pub fn commit(self) -> Result<()> {
        self.finalized.set(true);
        let sql = format!("RELEASE {}", quote_identifier(&self.name));
        self.tx.execute_batch(&sql).map_err(|e| {
            PoolError::Savepoint(format!(
                "failed to release savepoint '{}': {}",
                self.name, e
            ))
        })
    }

    /// Rollback to this savepoint and release it.
    ///
    /// This undoes all changes made since the savepoint was created,
    /// then releases the savepoint. The savepoint cannot be used after this.
    ///
    /// # Errors
    ///
    /// Returns an error if the ROLLBACK TO or RELEASE SQL commands fail.
    pub fn rollback(self) -> Result<()> {
        self.finalized.set(true);

        // Rollback to the savepoint
        let rollback_sql = format!("ROLLBACK TO {}", quote_identifier(&self.name));
        self.tx.execute_batch(&rollback_sql).map_err(|e| {
            PoolError::Savepoint(format!(
                "failed to rollback savepoint '{}': {}",
                self.name, e
            ))
        })?;

        // Release the savepoint
        let release_sql = format!("RELEASE {}", quote_identifier(&self.name));
        self.tx.execute_batch(&release_sql).map_err(|e| {
            PoolError::Savepoint(format!(
                "failed to release savepoint '{}' after rollback: {}",
                self.name, e
            ))
        })
    }

    /// Get a reference to the underlying transaction for executing queries.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use sqlite_kit::Savepoint;
    /// # fn example(tx: &rusqlite::Transaction) -> Result<(), sqlite_kit::PoolError> {
    /// let sp = Savepoint::new(tx, "sp1")?;
    /// sp.transaction().execute("INSERT INTO log (msg) VALUES (?)", ["test"])?;
    /// sp.commit()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn transaction(&self) -> &Transaction<'tx> {
        self.tx
    }

    /// Get the savepoint name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if the savepoint has been finalized (committed or rolled back).
    pub fn is_finalized(&self) -> bool {
        self.finalized.get()
    }
}

impl Drop for Savepoint<'_> {
    fn drop(&mut self) {
        if !self.finalized.get() {
            crate::telemetry::warn!(
                savepoint = %self.name,
                "Savepoint dropped without explicit commit or rollback"
            );
            // Note: We don't auto-rollback because Drop is sync and we can't
            // guarantee the connection is still valid. The transaction will
            // handle cleanup when it commits/rolls back.
        }
    }
}

/// Validate a savepoint name.
///
/// Valid names:
/// - Non-empty
/// - Alphanumeric characters and underscores only
/// - Cannot start with a digit
///
/// # Examples
///
/// ```rust,ignore
/// assert!(is_valid_savepoint_name("my_savepoint"));
/// assert!(is_valid_savepoint_name("sp1"));
/// assert!(is_valid_savepoint_name("_private"));
/// assert!(!is_valid_savepoint_name(""));
/// assert!(!is_valid_savepoint_name("1starts_with_digit"));
/// assert!(!is_valid_savepoint_name("has-dash"));
/// assert!(!is_valid_savepoint_name("has space"));
/// ```
fn is_valid_savepoint_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    // First character must not be a digit
    if let Some(first) = name.chars().next() {
        if first.is_ascii_digit() {
            return false;
        }
    }

    // All characters must be ASCII alphanumeric or underscore
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_valid_savepoint_names() {
        assert!(is_valid_savepoint_name("my_savepoint"));
        assert!(is_valid_savepoint_name("sp1"));
        assert!(is_valid_savepoint_name("MyMigration"));
        assert!(is_valid_savepoint_name("migration_v1"));
        assert!(is_valid_savepoint_name("a"));
        assert!(is_valid_savepoint_name("_private"));
        assert!(is_valid_savepoint_name("UPPERCASE"));
    }

    #[test]
    fn test_invalid_savepoint_names() {
        assert!(!is_valid_savepoint_name(""));
        assert!(!is_valid_savepoint_name("1starts_with_digit"));
        assert!(!is_valid_savepoint_name("has space"));
        assert!(!is_valid_savepoint_name("has-dash"));
        assert!(!is_valid_savepoint_name("has;semicolon"));
        assert!(!is_valid_savepoint_name("has'quote"));
        assert!(!is_valid_savepoint_name("DROP TABLE users;--"));
    }

    #[test]
    fn test_savepoint_commit() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .unwrap();

        let tx = conn.transaction().unwrap();

        // Insert before savepoint
        tx.execute("INSERT INTO test (value) VALUES (?)", ["before"])
            .unwrap();

        // Create savepoint and insert
        {
            let sp = Savepoint::new(&tx, "sp1").unwrap();
            sp.transaction()
                .execute("INSERT INTO test (value) VALUES (?)", ["during"])
                .unwrap();
            sp.commit().unwrap();
        }

        tx.commit().unwrap();

        // Both rows should exist
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM test", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_savepoint_rollback() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .unwrap();

        let tx = conn.transaction().unwrap();

        // Insert before savepoint
        tx.execute("INSERT INTO test (value) VALUES (?)", ["before"])
            .unwrap();

        // Create savepoint, insert, and rollback
        {
            let sp = Savepoint::new(&tx, "sp1").unwrap();
            sp.transaction()
                .execute("INSERT INTO test (value) VALUES (?)", ["during"])
                .unwrap();
            sp.rollback().unwrap();
        }

        tx.commit().unwrap();

        // Only first row should exist
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM test", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let value: String = conn
            .query_row("SELECT value FROM test", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "before");
    }

    #[test]
    fn test_nested_savepoints() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .unwrap();

        let tx = conn.transaction().unwrap();

        // Insert row 1 (base transaction)
        tx.execute("INSERT INTO test (value) VALUES (?)", ["base"])
            .unwrap();

        // Outer savepoint - insert row 2
        {
            let outer = Savepoint::new(&tx, "outer_sp").unwrap();
            outer
                .transaction()
                .execute("INSERT INTO test (value) VALUES (?)", ["outer"])
                .unwrap();

            // Inner savepoint - insert row 3
            {
                let inner = Savepoint::new(&tx, "inner_sp").unwrap();
                inner
                    .transaction()
                    .execute("INSERT INTO test (value) VALUES (?)", ["inner"])
                    .unwrap();
                // Rollback inner - row 3 is undone
                inner.rollback().unwrap();
            }

            // Commit outer - rows 1 and 2 persist
            outer.commit().unwrap();
        }

        tx.commit().unwrap();

        // Only rows 1 and 2 should exist
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM test", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        let values: Vec<String> = conn
            .prepare("SELECT value FROM test ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(values, vec!["base", "outer"]);
    }

    #[test]
    fn test_savepoint_invalid_name() {
        let mut conn = Connection::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();

        let result = Savepoint::new(&tx, "");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid savepoint name"));

        let result = Savepoint::new(&tx, "1invalid");
        assert!(result.is_err());

        let result = Savepoint::new(&tx, "has space");
        assert!(result.is_err());
    }

    #[test]
    fn test_savepoint_accessors() {
        let mut conn = Connection::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();

        let sp = Savepoint::new(&tx, "test_sp").unwrap();
        assert_eq!(sp.name(), "test_sp");
        assert!(!sp.is_finalized());

        sp.commit().unwrap();
        // Can't check is_finalized after commit because sp is moved
    }
}
