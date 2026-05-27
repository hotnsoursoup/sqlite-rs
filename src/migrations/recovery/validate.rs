//! Schema integrity validation.
//!
//! Read-only checks: detect missing tables/columns, verify SQLite's own
//! integrity check passes, and surface foreign-key violations. None of these
//! mutate the database.

use crate::error::Result;
use crate::sql::quote_identifier;
use rusqlite::Connection;

/// Schema integrity issue.
#[derive(Debug, Clone)]
pub enum SchemaIssue {
    /// Expected table is missing.
    MissingTable(String),
    /// Table exists but expected column is missing.
    MissingColumn { table: String, column: String },
    /// Schema version mismatch.
    VersionMismatch { expected: String, actual: String },
    /// Schema meta table is missing.
    NoVersionTracking,
    /// Foreign key violation found.
    ForeignKeyViolation { table: String, rowid: i64 },
    /// Integrity check failed.
    IntegrityCheckFailed(String),
}

/// Validate schema integrity.
///
/// Checks that:
/// - All expected tables exist
/// - Schema version is tracked
/// - Foreign key constraints are satisfied
/// - SQLite integrity check passes
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `expected_tables` - List of tables that should exist
///
/// # Returns
///
/// List of integrity issues found. Empty list means schema is valid.
///
/// # Example
///
/// ```rust,no_run
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_rs::PoolError> {
/// let issues = sqlite_rs::validate_schema_integrity(conn, &["users", "posts", "comments"])?;
/// if issues.is_empty() {
///     println!("Schema is valid");
/// } else {
///     for issue in &issues {
///         println!("Issue: {:?}", issue);
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub fn validate_schema_integrity(
    conn: &Connection,
    expected_tables: &[&str],
) -> Result<Vec<SchemaIssue>> {
    let mut issues = Vec::new();

    let has_version_tracking: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_meta'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !has_version_tracking {
        issues.push(SchemaIssue::NoVersionTracking);
    }

    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;

    let existing_tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    for table in expected_tables {
        if !existing_tables.contains(&table.to_string()) {
            issues.push(SchemaIssue::MissingTable(table.to_string()));
        }
    }

    let integrity_result: String = conn
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .unwrap_or_else(|_| "error".to_string());

    if integrity_result != "ok" {
        issues.push(SchemaIssue::IntegrityCheckFailed(integrity_result));
    }

    let mut fk_stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let fk_violations: Vec<(String, i64)> = fk_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (table, rowid) in fk_violations {
        issues.push(SchemaIssue::ForeignKeyViolation { table, rowid });
    }

    Ok(issues)
}

/// Validate that specific columns exist in tables.
///
/// # Example
///
/// ```rust,no_run
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_rs::PoolError> {
/// let issues = sqlite_rs::validate_table_columns(conn, &[
///     ("users", &["id", "name", "email"]),
///     ("posts", &["id", "user_id", "title", "content"]),
/// ])?;
/// # Ok(())
/// # }
/// ```
pub fn validate_table_columns(
    conn: &Connection,
    table_columns: &[(&str, &[&str])],
) -> Result<Vec<SchemaIssue>> {
    let mut issues = Vec::new();

    for (table, columns) in table_columns {
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !table_exists {
            issues.push(SchemaIssue::MissingTable(table.to_string()));
            continue;
        }

        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))?;
        let existing_columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();

        for column in *columns {
            if !existing_columns.contains(&column.to_string()) {
                issues.push(SchemaIssue::MissingColumn {
                    table: table.to_string(),
                    column: column.to_string(),
                });
            }
        }
    }

    Ok(issues)
}
