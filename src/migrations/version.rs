//! Version comparison and schema state detection.

use crate::error::Result;
use rusqlite::Connection;

/// Current state of the database schema.
#[derive(Debug, Clone)]
pub enum SchemaState {
    /// Database is empty (no tables).
    Empty,
    /// Schema is complete with the given version.
    Complete(String),
    /// Schema is partially applied (some tables exist).
    Partial {
        existing_tables: Vec<String>,
        missing_tables: Vec<String>,
    },
    /// Schema has an unknown version (possibly newer than code).
    Unknown(String),
    /// Schema integrity check failed.
    Corrupted(String),
}

/// Get the current schema version from the database.
///
/// Returns `None` if no version is recorded (fresh database).
///
/// # Example
///
/// ```rust,no_run
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// if let Some(version) = sqlite_kit::get_current_version(conn)? {
///     println!("Current schema version: {}", version);
/// } else {
///     println!("No schema version recorded (fresh database)");
/// }
/// # Ok(())
/// # }
/// ```
pub fn get_current_version(conn: &Connection) -> Result<Option<String>> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(version)
}

/// Detect the current schema state.
///
/// Analyzes the database to determine:
/// - If it's empty (no tables)
/// - If the schema is complete (all expected tables exist)
/// - If the schema is partial (some tables missing)
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::{detect_schema_state, SchemaState};
///
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// let state = detect_schema_state(conn, &["users", "posts", "comments"])?;
/// match state {
///     SchemaState::Empty => println!("Fresh database"),
///     SchemaState::Complete(v) => println!("Schema v{} is complete", v),
///     SchemaState::Partial { missing_tables, .. } => {
///         println!("Missing tables: {:?}", missing_tables);
///     }
///     _ => {}
/// }
/// # Ok(())
/// # }
/// ```
pub fn detect_schema_state(conn: &Connection, expected_tables: &[&str]) -> Result<SchemaState> {
    // Check if schema_meta table exists and get version
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .ok();

    // Get list of existing tables
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;

    let existing_tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    if existing_tables.is_empty() {
        return Ok(SchemaState::Empty);
    }

    if let Some(version) = version {
        // Check for missing tables
        let missing: Vec<String> = expected_tables
            .iter()
            .filter(|t| !existing_tables.contains(&t.to_string()))
            .map(|s| s.to_string())
            .collect();

        if missing.is_empty() {
            Ok(SchemaState::Complete(version))
        } else {
            Ok(SchemaState::Partial {
                existing_tables,
                missing_tables: missing,
            })
        }
    } else {
        // Tables exist but no version - partial state
        let missing: Vec<String> = expected_tables
            .iter()
            .filter(|t| !existing_tables.contains(&t.to_string()))
            .map(|s| s.to_string())
            .collect();

        Ok(SchemaState::Partial {
            existing_tables,
            missing_tables: missing,
        })
    }
}

/// Compare semantic versions.
///
/// Returns:
/// - `-1` if a < b
/// - `0` if a == b
/// - `1` if a > b
///
/// # Example
///
/// ```rust
/// use sqlite_kit::compare_versions;
///
/// assert!(compare_versions("1.0.0", "1.0.1") < 0);
/// assert!(compare_versions("2.0.0", "1.9.9") > 0);
/// assert!(compare_versions("1.0.0", "1.0.0") == 0);
/// ```
pub fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |v: &str| -> Vec<u32> { v.split('.').filter_map(|s| s.parse().ok()).collect() };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for i in 0..3 {
        let a_val = a_parts.get(i).copied().unwrap_or(0);
        let b_val = b_parts.get(i).copied().unwrap_or(0);

        if a_val < b_val {
            return -1;
        }
        if a_val > b_val {
            return 1;
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), 0);
        assert_eq!(compare_versions("1.0.0", "1.0.1"), -1);
        assert_eq!(compare_versions("1.0.1", "1.0.0"), 1);
        assert_eq!(compare_versions("1.1.0", "1.0.9"), 1);
        assert_eq!(compare_versions("2.0.0", "1.9.9"), 1);
    }
}
