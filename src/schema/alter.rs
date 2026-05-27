//! Safe schema alteration utilities.
//!
//! These functions handle the "already exists" case gracefully,
//! making migrations idempotent and safe to run multiple times.

use super::introspection::{column_exists, index_exists};
use crate::sql::quote_identifier;
use rusqlite::{Connection, Result};

/// Definition for a new column to add.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    /// Column name.
    pub name: String,
    /// Column type (e.g., "TEXT", "INTEGER", "REAL", "BLOB").
    pub column_type: String,
    /// Whether the column allows NULL values.
    pub nullable: bool,
    /// Default value expression (e.g., "'active'", "0", "datetime('now')").
    pub default: Option<String>,
}

impl ColumnDef {
    /// Create a new column definition.
    pub fn new(name: impl Into<String>, column_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            column_type: column_type.into(),
            nullable: true,
            default: None,
        }
    }

    /// Set the column as NOT NULL.
    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    /// Set a default value for the column.
    pub fn default(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self
    }

    /// Build the SQL fragment for this column definition.
    fn to_sql(&self) -> String {
        let mut sql = format!("{} {}", quote_identifier(&self.name), self.column_type);

        if !self.nullable {
            sql.push_str(" NOT NULL");
        }

        if let Some(ref default) = self.default {
            sql.push_str(&format!(" DEFAULT {}", default));
        }

        sql
    }
}

/// Definition for a new index to create.
#[derive(Debug, Clone)]
pub struct IndexDef {
    /// Index name.
    pub name: String,
    /// Table to create the index on.
    pub table: String,
    /// Columns to include in the index.
    pub columns: Vec<String>,
    /// Whether this is a unique index.
    pub unique: bool,
    /// Optional WHERE clause for partial indexes.
    pub where_clause: Option<String>,
}

impl IndexDef {
    /// Create a new index definition.
    pub fn new(name: impl Into<String>, table: impl Into<String>, columns: &[&str]) -> Self {
        Self {
            name: name.into(),
            table: table.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique: false,
            where_clause: None,
        }
    }

    /// Make this a unique index.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Add a WHERE clause for a partial index.
    pub fn where_clause(mut self, clause: impl Into<String>) -> Self {
        self.where_clause = Some(clause.into());
        self
    }

    /// Build the CREATE INDEX SQL statement.
    fn to_sql(&self) -> String {
        let unique = if self.unique { "UNIQUE " } else { "" };
        let escaped_columns: Vec<String> =
            self.columns.iter().map(|c| quote_identifier(c)).collect();
        let columns = escaped_columns.join(", ");

        let mut sql = format!(
            "CREATE {}INDEX {} ON {}({})",
            unique,
            quote_identifier(&self.name),
            quote_identifier(&self.table),
            columns
        );

        if let Some(ref where_clause) = self.where_clause {
            sql.push_str(&format!(" WHERE {}", where_clause));
        }

        sql
    }
}

/// Safely add a column to a table.
pub fn safe_add_column(conn: &Connection, table: &str, column: ColumnDef) -> Result<bool> {
    // Check if column already exists
    if column_exists(conn, table, &column.name)? {
        return Ok(false);
    }

    // Add the column - catch "duplicate column" error for TOCTOU safety
    let sql = format!(
        "ALTER TABLE {} ADD COLUMN {}",
        quote_identifier(table),
        column.to_sql()
    );
    match conn.execute(&sql, []) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") =>
        {
            // Another transaction added the column between our check and execute
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Safely create an index.
pub fn safe_create_index(conn: &Connection, index: IndexDef) -> Result<bool> {
    // Check if index already exists
    if index_exists(conn, &index.name)? {
        return Ok(false);
    }

    // Create the index - catch "already exists" error for TOCTOU safety
    match conn.execute(&index.to_sql(), []) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg))) if msg.contains("already exists") => {
            // Another transaction created the index between our check and execute
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Safely drop a column from a table.
pub fn safe_drop_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    // Check if column exists
    if !column_exists(conn, table, column)? {
        return Ok(false);
    }

    // Drop the column (requires SQLite 3.35.0+)
    let sql = format!(
        "ALTER TABLE {} DROP COLUMN {}",
        quote_identifier(table),
        quote_identifier(column)
    );
    conn.execute(&sql, [])?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_add_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        // First add should succeed
        let added = safe_add_column(&conn, "test", ColumnDef::new("name", "TEXT")).unwrap();
        assert!(added);

        // Second add should return false (already exists)
        let added = safe_add_column(&conn, "test", ColumnDef::new("name", "TEXT")).unwrap();
        assert!(!added);

        // Verify column exists
        assert!(column_exists(&conn, "test", "name").unwrap());
    }

    #[test]
    fn test_safe_add_column_with_default() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        // Add column with default
        safe_add_column(
            &conn,
            "test",
            ColumnDef::new("status", "TEXT").default("'pending'"),
        )
        .unwrap();

        // Insert row and verify default is applied
        conn.execute("INSERT INTO test (id) VALUES (1)", [])
            .unwrap();
        let status: String = conn
            .query_row("SELECT status FROM test WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn test_safe_create_index() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, email TEXT)", [])
            .unwrap();

        // First create should succeed
        let created =
            safe_create_index(&conn, IndexDef::new("idx_test_email", "test", &["email"])).unwrap();
        assert!(created);

        // Second create should return false (already exists)
        let created =
            safe_create_index(&conn, IndexDef::new("idx_test_email", "test", &["email"])).unwrap();
        assert!(!created);

        // Verify index exists
        assert!(index_exists(&conn, "idx_test_email").unwrap());
    }

    #[test]
    fn test_column_def_builder() {
        let col = ColumnDef::new("email", "TEXT")
            .not_null()
            .default("'unknown'");

        assert_eq!(col.name, "email");
        assert_eq!(col.column_type, "TEXT");
        assert!(!col.nullable);
        assert_eq!(col.default, Some("'unknown'".to_string()));
        assert_eq!(col.to_sql(), "\"email\" TEXT NOT NULL DEFAULT 'unknown'");
    }

    #[test]
    fn test_index_def_builder() {
        let idx = IndexDef::new("idx_users_status", "users", &["status", "created_at"])
            .unique()
            .where_clause("deleted_at IS NULL");

        assert_eq!(
            idx.to_sql(),
            "CREATE UNIQUE INDEX \"idx_users_status\" ON \"users\"(\"status\", \"created_at\") WHERE deleted_at IS NULL"
        );
    }
}
