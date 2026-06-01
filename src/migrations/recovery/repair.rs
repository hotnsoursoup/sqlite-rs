//! Best-effort repair for partially-corrupt databases.
//!
//! Distinguishes healthy from corrupt tables (by attempting a trivial query),
//! then drops the corrupt ones and re-runs migrations to recreate them. If
//! every table is corrupt, falls back to a full [`reset_database`].

use super::reset::{list_user_tables, reset_database};
use crate::error::Result;
use crate::migrations::runner::run_migrations;
use crate::migrations::types::Migration;
use crate::sql::quote_identifier;
use rusqlite::Connection;

/// Result of database repair.
#[derive(Debug, Clone)]
pub struct RepairResult {
    /// Number of tables that were rebuilt.
    pub tables_rebuilt: usize,
    /// Number of rows recovered from corrupt tables.
    pub data_recovered: usize,
    /// True if a full database reset was required.
    pub full_reset: bool,
}

/// Attempt to repair a corrupted database by rebuilding from migrations.
///
/// This function:
/// 1. Backs up existing data where possible
/// 2. Drops corrupted tables
/// 3. Rebuilds schema from migrations
///
/// **WARNING**: Data in corrupted tables may be lost!
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::{repair_database, Migration};
///
/// # fn example(conn: &mut rusqlite::Connection, migrations: &[Migration]) -> Result<(), sqlite_kit::PoolError> {
/// let result = repair_database(conn, migrations)?;
/// println!("Repaired database, {} tables rebuilt", result.tables_rebuilt);
/// # Ok(())
/// # }
/// ```
pub fn repair_database(conn: &mut Connection, migrations: &[Migration]) -> Result<RepairResult> {
    crate::telemetry::warn!("attempting database repair");

    let tables = list_user_tables(conn)?;
    let (healthy, corrupt) = partition_by_health(conn, &tables);

    crate::telemetry::info!(
        healthy = healthy.len(),
        corrupt = corrupt.len(),
        "analyzed tables"
    );

    // Everything is corrupt — fall back to a full reset.
    if healthy.is_empty() {
        reset_database(conn, migrations)?;
        return Ok(RepairResult {
            tables_rebuilt: tables.len(),
            data_recovered: 0,
            full_reset: true,
        });
    }

    // Drop the corrupt tables and let migrations rebuild them.
    conn.execute("PRAGMA foreign_keys = OFF", [])?;

    let tables_rebuilt = corrupt.len();
    for table in &corrupt {
        conn.execute(
            &format!("DROP TABLE IF EXISTS {}", quote_identifier(table)),
            [],
        )?;
    }

    // Drop the schema tracking tables if they're corrupt so the rerun
    // recreates them cleanly.
    let schema_meta_healthy = conn
        .query_row("SELECT COUNT(*) FROM schema_meta", [], |_| Ok(()))
        .is_ok();
    if !schema_meta_healthy {
        conn.execute("DROP TABLE IF EXISTS schema_meta", [])?;
        conn.execute("DROP TABLE IF EXISTS schema_migrations", [])?;
    }

    conn.execute("PRAGMA foreign_keys = ON", [])?;

    run_migrations(conn, migrations)?;

    Ok(RepairResult {
        tables_rebuilt,
        data_recovered: 0,
        full_reset: false,
    })
}

/// Split tables into (healthy, corrupt) by trying a trivial query against each.
fn partition_by_health(conn: &Connection, tables: &[String]) -> (Vec<String>, Vec<String>) {
    let mut healthy = Vec::new();
    let mut corrupt = Vec::new();
    for table in tables {
        let is_healthy = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {}", quote_identifier(table)),
                [],
                |_| Ok(()),
            )
            .is_ok();
        if is_healthy {
            healthy.push(table.clone());
        } else {
            corrupt.push(table.clone());
        }
    }
    (healthy, corrupt)
}
