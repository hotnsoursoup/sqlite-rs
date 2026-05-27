//! Schema introspection utilities.
//!
//! Functions for querying database schema information at runtime.

use crate::sql::quote_identifier;
use rusqlite::{Connection, Result};

/// Information about an index from PRAGMA index_list + index_info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    /// Index name.
    pub name: String,
    /// Table the index belongs to.
    pub table: String,
    /// Columns in the index (in order).
    pub columns: Vec<String>,
    /// Whether this is a unique index.
    pub is_unique: bool,
    /// Whether this is the primary key index.
    pub is_primary: bool,
}

/// Complete table information including columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    /// Table name.
    pub name: String,
    /// Columns in the table.
    pub columns: Vec<ColumnInfo>,
}

impl TableInfo {
    /// Create a new TableInfo.
    pub fn new(name: impl Into<String>, columns: Vec<ColumnInfo>) -> Self {
        Self {
            name: name.into(),
            columns,
        }
    }

    /// Get a column by name.
    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Check if table has a column.
    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c.name == name)
    }

    /// Get primary key columns.
    pub fn primary_key_columns(&self) -> Vec<&ColumnInfo> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }

    /// Get all column names.
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}

/// Information about a table column from PRAGMA table_info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    /// Column index (0-based).
    pub cid: i32,
    /// Column name.
    pub name: String,
    /// Column type (e.g., "TEXT", "INTEGER").
    pub column_type: String,
    /// Whether the column has a NOT NULL constraint.
    pub not_null: bool,
    /// Default value expression, if any.
    pub default_value: Option<String>,
    /// Whether this column is part of the primary key.
    pub primary_key: bool,
}

/// Check if a table exists in the database.
pub fn table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    let count: i32 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table_name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Check if a column exists in a table.
pub fn column_exists(conn: &Connection, table_name: &str, column_name: &str) -> Result<bool> {
    let columns = get_table_columns(conn, table_name)?;
    Ok(columns.iter().any(|c| c == column_name))
}

/// Check if an index exists in the database.
pub fn index_exists(conn: &Connection, index_name: &str) -> Result<bool> {
    let count: i32 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
        [index_name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Get a list of column names for a table.
pub fn get_table_columns(conn: &Connection, table_name: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!(
        "PRAGMA table_info({})",
        quote_identifier(table_name)
    ))?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}

/// Get detailed information about all columns in a table.
pub fn get_table_info(conn: &Connection, table_name: &str) -> Result<Vec<ColumnInfo>> {
    let mut stmt = conn.prepare(&format!(
        "PRAGMA table_info({})",
        quote_identifier(table_name)
    ))?;
    let columns: Vec<ColumnInfo> = stmt
        .query_map([], |row| {
            Ok(ColumnInfo {
                cid: row.get(0)?,
                name: row.get(1)?,
                column_type: row.get(2)?,
                not_null: row.get::<_, i32>(3)? != 0,
                default_value: row.get(4)?,
                primary_key: row.get::<_, i32>(5)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}

/// List all user tables in the database.
pub fn list_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tables)
}

/// Get index information for a table.
pub fn get_index_info(conn: &Connection, table_name: &str) -> Result<Vec<IndexInfo>> {
    let mut stmt = conn.prepare(&format!(
        "PRAGMA index_list({})",
        quote_identifier(table_name)
    ))?;
    let index_rows: Vec<(String, bool, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i32>(2)? != 0,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut indexes = Vec::new();
    for (index_name, is_unique, origin) in index_rows {
        let mut col_stmt = conn.prepare(&format!(
            "PRAGMA index_info({})",
            quote_identifier(&index_name)
        ))?;
        let columns: Vec<String> = col_stmt
            .query_map([], |row| row.get::<_, String>(2))?
            .collect::<Result<Vec<_>, _>>()?;

        indexes.push(IndexInfo {
            name: index_name,
            table: table_name.to_string(),
            columns,
            is_unique,
            is_primary: origin == "pk",
        });
    }

    Ok(indexes)
}

/// Get complete table information including all columns.
pub fn get_full_table_info(conn: &Connection, table_name: &str) -> Result<TableInfo> {
    let columns = get_table_info(conn, table_name)?;
    Ok(TableInfo::new(table_name, columns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_exists() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        assert!(table_exists(&conn, "test").unwrap());
        assert!(!table_exists(&conn, "nonexistent").unwrap());
    }

    #[test]
    fn test_column_exists() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT)", [])
            .unwrap();
        assert!(column_exists(&conn, "test", "id").unwrap());
        assert!(column_exists(&conn, "test", "name").unwrap());
        assert!(!column_exists(&conn, "test", "email").unwrap());
    }

    #[test]
    fn test_get_table_info() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER DEFAULT 0)",
            [],
        )
        .unwrap();

        let info = get_table_info(&conn, "test").unwrap();
        assert_eq!(info.len(), 3);
        let id_col = info.iter().find(|c| c.name == "id").unwrap();
        assert!(id_col.primary_key);
    }

    #[test]
    fn test_list_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        conn.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        let tables = list_tables(&conn).unwrap();
        assert_eq!(tables.len(), 2);
    }

    #[test]
    fn test_get_index_info() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT)",
            [],
        )
        .unwrap();
        conn.execute("CREATE INDEX idx_email ON users(email)", [])
            .unwrap();
        let indexes = get_index_info(&conn, "users").unwrap();
        let email_idx = indexes.iter().find(|i| i.name == "idx_email").unwrap();
        assert!(!email_idx.is_unique);
        assert_eq!(email_idx.columns, vec!["email"]);
    }

    #[test]
    fn test_get_full_table_info() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
            .unwrap();
        let info = get_full_table_info(&conn, "users").unwrap();
        assert_eq!(info.name, "users");
        assert!(info.has_column("id"));
        assert!(info.has_column("name"));
        assert_eq!(info.primary_key_columns().len(), 1);
    }

    #[test]
    fn test_table_info_column_method() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT)", [])
            .unwrap();
        let info = get_full_table_info(&conn, "test").unwrap();
        assert!(info.column("id").is_some());
        assert!(info.column("nonexistent").is_none());
    }
}
