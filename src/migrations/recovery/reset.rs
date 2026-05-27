//! Destructive recovery: drop all tables and (optionally) re-run migrations.

use crate::error::{PoolError, Result};
use crate::migrations::runner::{run_migrations_with_options, MigrationOptions};
use crate::migrations::types::Migration;
use crate::sql::quote_identifier;
use rusqlite::Connection;

/// Reset the database by dropping all tables and re-running migrations.
///
/// **WARNING**: This destroys all data in the database!
///
/// # Arguments
///
/// * `conn` - Mutable database connection
/// * `migrations` - Migrations to apply after reset
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::{reset_database, Migration, MigrationKind};
///
/// const MIGRATIONS: &[Migration] = &[
///     Migration {
///         version: "1.0.0",
///         from_version: "0.0.0",
///         description: "Initial schema",
///         sql: "CREATE TABLE users (id INTEGER PRIMARY KEY);",
///         kind: MigrationKind::Baseline,
///         creates_tables: &["users"],
///     },
/// ];
///
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_rs::PoolError> {
/// // This will delete all data!
/// reset_database(conn, MIGRATIONS)?;
/// # Ok(())
/// # }
/// ```
pub fn reset_database(conn: &mut Connection, migrations: &[Migration]) -> Result<()> {
    reset_database_with_options(conn, migrations, MigrationOptions::new())
}

/// Reset database with custom migration options.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_rs::{reset_database_with_options, Migration, MigrationOptions};
///
/// # fn example(conn: &mut rusqlite::Connection, migrations: &[Migration]) -> Result<(), sqlite_rs::PoolError> {
/// reset_database_with_options(
///     conn,
///     migrations,
///     MigrationOptions::new().with_app_name("my_app")
/// )?;
/// # Ok(())
/// # }
/// ```
pub fn reset_database_with_options(
    conn: &mut Connection,
    migrations: &[Migration],
    options: MigrationOptions,
) -> Result<()> {
    crate::telemetry::warn!("resetting database - all data will be lost");

    // Disable FKs so drops can happen in any order.
    conn.execute("PRAGMA foreign_keys = OFF", [])?;

    let tables = list_user_tables(conn)?;
    for table in tables {
        crate::telemetry::debug!(table = %table, "dropping table");
        conn.execute(
            &format!("DROP TABLE IF EXISTS {}", quote_identifier(&table)),
            [],
        )
        .map_err(|e| PoolError::Migration(format!("failed to drop table {}: {}", table, e)))?;
    }

    // Drop views and triggers next so they don't reference missing tables.
    let mut view_stmt =
        conn.prepare("SELECT name, type FROM sqlite_master WHERE type IN ('view', 'trigger')")?;

    let objects: Vec<(String, String)> = view_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    drop(view_stmt);

    for (name, obj_type) in objects {
        let drop_sql = format!(
            "DROP {} IF EXISTS {}",
            obj_type.to_uppercase(),
            quote_identifier(&name)
        );
        conn.execute(&drop_sql, [])?;
    }

    conn.execute("PRAGMA foreign_keys = ON", [])?;

    crate::telemetry::info!("database reset complete, running migrations");

    run_migrations_with_options(conn, migrations, options)?;
    Ok(())
}

/// Drop all user tables without running migrations.
///
/// This is useful for testing or when you want to manually rebuild the schema.
///
/// # Example
///
/// ```rust,no_run
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_rs::PoolError> {
/// sqlite_rs::drop_all_tables(conn)?;
/// # Ok(())
/// # }
/// ```
pub fn drop_all_tables(conn: &mut Connection) -> Result<usize> {
    conn.execute("PRAGMA foreign_keys = OFF", [])?;

    let tables = list_user_tables(conn)?;
    let count = tables.len();

    for table in tables {
        conn.execute(
            &format!("DROP TABLE IF EXISTS {}", quote_identifier(&table)),
            [],
        )?;
    }

    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(count)
}

/// List all user tables in the database (excluding SQLite internal tables).
pub(super) fn list_user_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;

    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(tables)
}
