//! Migration runner tests.

use super::*;
use crate::migrations::types::MigrationKind;
use rusqlite::Connection;

const TEST_MIGRATIONS: &[Migration] = &[
    Migration {
        version: "1.0.0",
        from_version: "0.0.0",
        description: "Initial schema",
        sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
        kind: MigrationKind::Baseline,
        creates_tables: &["users"],
    },
    Migration {
        version: "1.1.0",
        from_version: "1.0.0",
        description: "Add email column",
        sql: "ALTER TABLE users ADD COLUMN email TEXT;",
        kind: MigrationKind::Incremental,
        creates_tables: &[],
    },
    Migration {
        version: "1.2.0",
        from_version: "1.1.0",
        description: "Add posts table",
        sql: "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);",
        kind: MigrationKind::Incremental,
        creates_tables: &["posts"],
    },
];
#[test]
fn test_migration_options_default_matches_new() {
    let options = MigrationOptions::default();

    assert!(!options.dry_run);
    assert!(options.validate_creates);
    assert!(options.app_name.is_none());
}

#[test]
fn test_run_migrations() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    let version: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "1.2.0");

    let users_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = 'users'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    assert!(users_exists);
}

#[test]
fn test_dry_run() {
    let mut conn = Connection::open_in_memory().unwrap();
    let result = run_migrations_with_options(
        &mut conn,
        TEST_MIGRATIONS,
        MigrationOptions::new().dry_run(),
    )
    .unwrap();

    assert!(result.dry_run);
    assert_eq!(result.applied_count, 3);

    let table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = 'users'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    assert!(!table_exists);
}

#[test]
fn test_migration_history() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    let history = get_migration_history(&conn).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].version, "1.0.0");
    assert_eq!(history[1].version, "1.1.0");
    assert_eq!(history[2].version, "1.2.0");
    assert!(history[0].success);
}

#[test]
fn test_is_migration_applied() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    assert!(is_migration_applied(&conn, "1.0.0").unwrap());
    assert!(is_migration_applied(&conn, "1.2.0").unwrap());
    assert!(!is_migration_applied(&conn, "2.0.0").unwrap());
}

#[test]
fn test_pending_migrations() {
    let mut conn = Connection::open_in_memory().unwrap();

    let pending = get_pending_migrations(&conn, TEST_MIGRATIONS).unwrap();
    assert_eq!(pending.len(), 3);

    run_migrations(&mut conn, TEST_MIGRATIONS).unwrap();

    let pending = get_pending_migrations(&conn, TEST_MIGRATIONS).unwrap();
    assert!(pending.is_empty());
}

#[test]
fn test_creates_tables_validation() {
    let bad_migrations: &[Migration] = &[Migration {
        version: "1.0.0",
        from_version: "0.0.0",
        description: "Broken migration",
        sql: "SELECT 1;", // Doesn't create the table it claims to
        kind: MigrationKind::Baseline,
        creates_tables: &["nonexistent"],
    }];

    let mut conn = Connection::open_in_memory().unwrap();
    let result = run_migrations(&mut conn, bad_migrations);

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("doesn't exist after execution"));
}

#[test]
fn test_app_name_metadata() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations_with_options(
        &mut conn,
        TEST_MIGRATIONS,
        MigrationOptions::new().with_app_name("test_app"),
    )
    .unwrap();

    let app_name: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'app_name'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(app_name, "test_app");
}

#[test]
fn test_concurrent_migration_locking() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let temp = tempfile::NamedTempFile::new().unwrap();
    let path = temp.path().to_str().unwrap().to_string();

    let barrier = Arc::new(Barrier::new(2));
    let path1 = path.clone();
    let path2 = path.clone();

    let barrier1 = Arc::clone(&barrier);
    let barrier2 = Arc::clone(&barrier);

    // Thread 1: take BEGIN IMMEDIATE and hold it briefly.
    let handle1 = thread::spawn(move || {
        let conn = Connection::open(&path1).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        barrier1.wait();
        thread::sleep(std::time::Duration::from_millis(100));
        conn.execute_batch("ROLLBACK").unwrap();
    });

    // Thread 2: try to acquire the same lock; it should block until thread 1 releases.
    let handle2 = thread::spawn(move || {
        barrier2.wait();
        thread::sleep(std::time::Duration::from_millis(10));

        let start = std::time::Instant::now();
        let conn = Connection::open(&path2).unwrap();
        let result = conn.execute_batch("BEGIN IMMEDIATE");
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() >= 50,
            "Should have blocked, but only waited {:?}",
            elapsed
        );

        result
    });

    handle1.join().unwrap();
    let result2 = handle2.join().unwrap();

    assert!(
        result2.is_ok(),
        "Thread 2 should acquire lock after thread 1 releases it"
    );
}
