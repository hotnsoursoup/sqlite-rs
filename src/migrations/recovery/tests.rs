//! Tests covering validate / reset / repair flows end-to-end.

use super::*;
use crate::migrations::runner::run_migrations;
use crate::migrations::types::{Migration, MigrationKind};
use rusqlite::Connection;

const TEST_MIGRATIONS: &[Migration] = &[Migration {
    version: "1.0.0",
    from_version: "0.0.0",
    description: "Initial schema",
    sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT);
              CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id), title TEXT);",
    kind: MigrationKind::Baseline,
    creates_tables: &["users", "posts"],
}];

#[test]
fn test_validate_schema_integrity_empty() {
    let conn = Connection::open_in_memory().unwrap();
    let issues = validate_schema_integrity(&conn, &["users", "posts"]).unwrap();

    assert!(issues
        .iter()
        .any(|i| matches!(i, SchemaIssue::NoVersionTracking)));
    assert_eq!(
        issues
            .iter()
            .filter(|i| matches!(i, SchemaIssue::MissingTable(_)))
            .count(),
        2
    );
}

#[test]
fn test_validate_schema_integrity_complete() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    let issues = validate_schema_integrity(&conn, &["users", "posts"]).unwrap();
    assert!(issues.is_empty());
}

#[test]
fn test_validate_table_columns() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    // Valid columns
    let issues = validate_table_columns(
        &conn,
        &[
            ("users", &["id", "name", "email"]),
            ("posts", &["id", "user_id", "title"]),
        ],
    )
    .unwrap();
    assert!(issues.is_empty());

    // Missing column
    let issues =
        validate_table_columns(&conn, &[("users", &["id", "name", "nonexistent"])]).unwrap();
    assert!(issues.iter().any(
        |i| matches!(i, SchemaIssue::MissingColumn { column, .. } if column == "nonexistent")
    ));
}

#[test]
fn test_reset_database() {
    let mut conn = Connection::open_in_memory().unwrap();

    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    conn.execute(
        "INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')",
        [],
    )
    .unwrap();

    reset_database(&mut conn, TEST_MIGRATIONS).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);

    let version: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "1.0.0");
}

#[test]
fn test_drop_all_tables() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    let dropped = drop_all_tables(&mut conn).unwrap();
    assert!(dropped >= 2); // At least users, posts, plus schema tables

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_drop_all_tables_escapes_identifiers() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE \"quote\"\"table\" (id INTEGER)", [])
        .unwrap();

    let dropped = drop_all_tables(&mut conn).unwrap();
    assert_eq!(dropped, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_repair_healthy_database() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();
    conn.execute(
        "INSERT INTO users (name, email) VALUES ('Bob', 'bob@example.com')",
        [],
    )
    .unwrap();

    let result = repair_database(&mut conn, TEST_MIGRATIONS).unwrap();
    assert_eq!(result.tables_rebuilt, 0);
    assert!(!result.full_reset);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
