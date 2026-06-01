//! Observer context for passing data through the observer chain.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// Type of database operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    /// SELECT query.
    Select,
    /// INSERT query.
    Insert,
    /// UPDATE query.
    Update,
    /// DELETE query.
    Delete,
    /// DDL operation (CREATE, ALTER, DROP).
    Ddl,
    /// Transaction control (BEGIN, COMMIT, ROLLBACK).
    Transaction,
    /// Other/unknown operation.
    Other,
}

impl OperationKind {
    /// Get string representation of the operation.
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationKind::Select => "select",
            OperationKind::Insert => "insert",
            OperationKind::Update => "update",
            OperationKind::Delete => "delete",
            OperationKind::Ddl => "ddl",
            OperationKind::Transaction => "transaction",
            OperationKind::Other => "other",
        }
    }

    /// Detect operation type from SQL string.
    pub fn from_sql(sql: &str) -> Self {
        let trimmed = sql.trim_start().to_uppercase();
        if trimmed.starts_with("WITH") {
            // WITH can be used with SELECT, INSERT, UPDATE, DELETE
            // Look for the main operation after the CTE
            if trimmed.contains(") INSERT") {
                OperationKind::Insert
            } else if trimmed.contains(") UPDATE") {
                OperationKind::Update
            } else if trimmed.contains(") DELETE") {
                OperationKind::Delete
            } else {
                // Default to SELECT for WITH ... SELECT or ambiguous cases
                OperationKind::Select
            }
        } else if trimmed.starts_with("SELECT") {
            OperationKind::Select
        } else if trimmed.starts_with("INSERT") {
            OperationKind::Insert
        } else if trimmed.starts_with("UPDATE") {
            OperationKind::Update
        } else if trimmed.starts_with("DELETE") {
            OperationKind::Delete
        } else if trimmed.starts_with("CREATE")
            || trimmed.starts_with("ALTER")
            || trimmed.starts_with("DROP")
        {
            OperationKind::Ddl
        } else if trimmed.starts_with("BEGIN")
            || trimmed.starts_with("COMMIT")
            || trimmed.starts_with("ROLLBACK")
            || trimmed.starts_with("SAVEPOINT")
            || trimmed.starts_with("RELEASE")
        {
            OperationKind::Transaction
        } else {
            OperationKind::Other
        }
    }
}

/// Context passed through the observer chain.
///
/// Contains information about the current operation and allows
/// observers to store and share data.
#[derive(Debug)]
pub struct OperationContext {
    /// SQL query or operation label.
    sql: String,
    /// Detected operation type.
    operation: OperationKind,
    /// Whether this is a read-only operation.
    is_read_only: bool,
    /// Custom extension data (type-safe key-value storage).
    extensions: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
}

impl OperationContext {
    /// Create a new context for a SQL query.
    ///
    /// Automatically detects the operation type from SQL.
    pub fn new(sql: impl Into<String>) -> Self {
        let sql = sql.into();
        let operation = OperationKind::from_sql(&sql);
        let is_read_only = matches!(operation, OperationKind::Select);

        Self {
            sql,
            operation,
            is_read_only,
            extensions: HashMap::new(),
        }
    }

    /// Create a context for a pool-level operation.
    ///
    /// Use this for operations like "pool:read" or "pool:write"
    /// where there's no specific SQL query.
    pub fn for_pool_operation(
        operation_label: impl Into<String>,
        operation: OperationKind,
        is_read_only: bool,
    ) -> Self {
        Self {
            sql: operation_label.into(),
            operation,
            is_read_only,
            extensions: HashMap::new(),
        }
    }

    /// Get the SQL query or operation label.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Get the operation type.
    pub fn operation(&self) -> OperationKind {
        self.operation
    }

    /// Check if this is a read-only operation.
    pub fn is_read_only(&self) -> bool {
        self.is_read_only
    }

    /// Set or replace the SQL query.
    ///
    /// Also re-detects the operation type.
    pub fn set_sql(&mut self, sql: impl Into<String>) {
        self.sql = sql.into();
        self.operation = OperationKind::from_sql(&self.sql);
        self.is_read_only = matches!(self.operation, OperationKind::Select);
    }

    /// Store a value in the context (type-safe).
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlite_kit::observer::OperationContext;
    /// let mut ctx = OperationContext::new("SELECT 1");
    /// ctx.set("start_time", 12345u64);
    /// ```
    pub fn set<T: Any + Send + Sync + 'static>(&mut self, key: &'static str, value: T) {
        self.extensions.insert(key, Arc::new(value));
    }

    /// Retrieve a value from the context with type checking.
    ///
    /// Returns `None` if the key doesn't exist or the type doesn't match.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlite_kit::observer::OperationContext;
    /// let mut ctx = OperationContext::new("SELECT 1");
    /// ctx.set("count", 42u32);
    /// assert_eq!(ctx.get::<u32>("count"), Some(&42));
    /// assert_eq!(ctx.get::<String>("count"), None); // Wrong type
    /// ```
    pub fn get<T: Any + Send + Sync + 'static>(&self, key: &'static str) -> Option<&T> {
        self.extensions.get(key).and_then(|v| v.downcast_ref::<T>())
    }

    /// Check if a key exists in the context.
    pub fn contains(&self, key: &'static str) -> bool {
        self.extensions.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_detection() {
        assert_eq!(
            OperationKind::from_sql("SELECT * FROM users"),
            OperationKind::Select
        );
        assert_eq!(
            OperationKind::from_sql("  select id from users"),
            OperationKind::Select
        );
        assert_eq!(
            OperationKind::from_sql("WITH cte AS (SELECT 1) SELECT * FROM cte"),
            OperationKind::Select
        );
        assert_eq!(
            OperationKind::from_sql("INSERT INTO users VALUES (1)"),
            OperationKind::Insert
        );
        assert_eq!(
            OperationKind::from_sql("UPDATE users SET name = 'x'"),
            OperationKind::Update
        );
        assert_eq!(
            OperationKind::from_sql("DELETE FROM users"),
            OperationKind::Delete
        );
        assert_eq!(
            OperationKind::from_sql("CREATE TABLE test (id INT)"),
            OperationKind::Ddl
        );
        assert_eq!(
            OperationKind::from_sql("ALTER TABLE test ADD col TEXT"),
            OperationKind::Ddl
        );
        assert_eq!(
            OperationKind::from_sql("DROP TABLE test"),
            OperationKind::Ddl
        );
        assert_eq!(
            OperationKind::from_sql("BEGIN TRANSACTION"),
            OperationKind::Transaction
        );
        assert_eq!(
            OperationKind::from_sql("COMMIT"),
            OperationKind::Transaction
        );
        assert_eq!(
            OperationKind::from_sql("PRAGMA table_info(test)"),
            OperationKind::Other
        );
    }

    #[test]
    fn test_context_new() {
        let ctx = OperationContext::new("SELECT 1");
        assert_eq!(ctx.sql(), "SELECT 1");
        assert_eq!(ctx.operation(), OperationKind::Select);
        assert!(ctx.is_read_only());
    }

    #[test]
    fn test_context_for_pool_operation() {
        let ctx = OperationContext::for_pool_operation("pool:write", OperationKind::Other, false);
        assert_eq!(ctx.sql(), "pool:write");
        assert_eq!(ctx.operation(), OperationKind::Other);
        assert!(!ctx.is_read_only());
    }

    #[test]
    fn test_context_set_sql() {
        let mut ctx = OperationContext::new("SELECT 1");
        assert!(ctx.is_read_only());

        ctx.set_sql("INSERT INTO test VALUES (1)");
        assert_eq!(ctx.operation(), OperationKind::Insert);
        assert!(!ctx.is_read_only());
    }

    #[test]
    fn test_context_extensions() {
        let mut ctx = OperationContext::new("SELECT 1");

        // Set and get value
        ctx.set("count", 42u32);
        assert_eq!(ctx.get::<u32>("count"), Some(&42));

        // Wrong type returns None
        assert_eq!(ctx.get::<String>("count"), None);

        // Missing key returns None
        assert_eq!(ctx.get::<u32>("missing"), None);

        // Contains check
        assert!(ctx.contains("count"));
        assert!(!ctx.contains("missing"));
    }

    #[test]
    fn test_operation_as_str() {
        assert_eq!(OperationKind::Select.as_str(), "select");
        assert_eq!(OperationKind::Insert.as_str(), "insert");
        assert_eq!(OperationKind::Update.as_str(), "update");
        assert_eq!(OperationKind::Delete.as_str(), "delete");
        assert_eq!(OperationKind::Ddl.as_str(), "ddl");
        assert_eq!(OperationKind::Transaction.as_str(), "transaction");
        assert_eq!(OperationKind::Other.as_str(), "other");
    }
}
