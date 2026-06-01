//! Migration execution logic.

use super::types::{Migration, MigrationKind};
use super::version::compare_versions;
use crate::error::{PoolError, Result};
use rusqlite::Connection;

/// Options for migration execution.
#[derive(Debug, Clone)]
pub struct MigrationOptions {
    /// If true, only preview migrations without applying them.
    pub dry_run: bool,
    /// If true, validate that `creates_tables` actually exist after each migration.
    pub validate_creates: bool,
    /// Application name to store in schema_meta (for multi-project support).
    pub app_name: Option<String>,
}

impl Default for MigrationOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl MigrationOptions {
    /// Create default options (apply migrations, validate creates).
    pub fn new() -> Self {
        Self {
            dry_run: false,
            validate_creates: true,
            app_name: None,
        }
    }

    /// Enable dry-run mode (preview only).
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// Set application name for multi-project databases.
    pub fn with_app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }

    /// Disable creates_tables validation.
    pub fn skip_validation(mut self) -> Self {
        self.validate_creates = false;
        self
    }
}

/// Result of migration execution.
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Number of migrations applied.
    pub applied_count: usize,
    /// List of applied migration versions.
    pub applied_versions: Vec<String>,
    /// Final schema version.
    pub final_version: String,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

/// A record of an applied migration.
#[derive(Debug, Clone)]
pub struct MigrationRecord {
    /// Migration version (e.g., "1.0.0").
    pub version: String,
    /// Description of the migration.
    pub description: String,
    /// Timestamp when the migration was applied.
    pub applied_at: String,
    /// Time taken to apply the migration in milliseconds.
    pub duration_ms: i64,
    /// Whether the migration was successful.
    pub success: bool,
}

/// Run migrations on a connection.
///
/// This function applies migrations in order, using savepoints for atomic rollback.
///
/// # Migration Order
///
/// 1. If starting fresh (version "0.0.0"), applies the baseline migration first
/// 2. Applies incremental migrations in version order
/// 3. Only applies migrations where `from_version <= current_version < version`
///
/// # Tables Created
///
/// - `schema_meta`: Stores current schema version and app metadata
/// - `schema_migrations`: Records all applied migrations
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::{Migration, MigrationKind, run_migrations};
///
/// const MIGRATIONS: &[Migration] = &[
///     Migration {
///         version: "1.0.0",
///         from_version: "0.0.0",
///         description: "Initial schema",
///         sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
///         kind: MigrationKind::Baseline,
///         creates_tables: &["users"],
///     },
/// ];
///
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// run_migrations(conn, MIGRATIONS)?;
/// # Ok(())
/// # }
/// ```
pub fn run_migrations(conn: &mut Connection, migrations: &[Migration]) -> Result<()> {
    run_migrations_with_options(conn, migrations, MigrationOptions::new())?;
    Ok(())
}

/// Run migrations with custom options.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::{Migration, MigrationOptions, run_migrations_with_options};
///
/// # fn example(conn: &mut rusqlite::Connection, migrations: &[Migration]) -> Result<(), sqlite_kit::PoolError> {
/// // Dry run to preview changes
/// let result = run_migrations_with_options(
///     conn,
///     migrations,
///     MigrationOptions::new().dry_run()
/// )?;
/// println!("Would apply {} migrations", result.applied_count);
/// # Ok(())
/// # }
/// ```
pub fn run_migrations_with_options(
    conn: &mut Connection,
    migrations: &[Migration],
    options: MigrationOptions,
) -> Result<MigrationResult> {
    // BEGIN IMMEDIATE takes a RESERVED lock so cross-process migrations
    // serialize at the database level. Dry runs don't take the lock.
    if !options.dry_run {
        crate::telemetry::info!("acquiring migration lock with BEGIN IMMEDIATE");
        conn.execute_batch("BEGIN IMMEDIATE")?;
    }

    let result = (|| {
        if !options.dry_run {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    updated_at TEXT DEFAULT (datetime('now'))
                );
                CREATE TABLE IF NOT EXISTS schema_migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT (datetime('now')),
                    description TEXT,
                    duration_ms INTEGER,
                    success INTEGER DEFAULT 1
                );",
            )?;

            if let Some(ref app_name) = options.app_name {
                conn.execute(
                    "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('app_name', ?1)",
                    rusqlite::params![app_name],
                )?;
            }
        }

        let current_version: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'version'",
                [],
                |row| row.get(0),
            )
            .ok();

        let current = current_version.as_deref().unwrap_or("0.0.0");

        crate::telemetry::info!(
            current_version = current,
            dry_run = options.dry_run,
            "checking migrations"
        );

        let mut applied_versions = Vec::new();

        // Baseline migration runs first on a fresh database.
        if current == "0.0.0" {
            if let Some(baseline) = migrations
                .iter()
                .find(|m| m.kind == MigrationKind::Baseline)
            {
                if options.dry_run {
                    applied_versions.push(baseline.version.to_string());
                } else {
                    apply_migration(conn, baseline, &options)?;
                    applied_versions.push(baseline.version.to_string());
                }
            }
        }

        let mut incremental: Vec<_> = migrations
            .iter()
            .filter(|m| m.kind == MigrationKind::Incremental)
            .collect();
        incremental.sort_by(|a, b| compare_versions(a.version, b.version).cmp(&0));

        for migration in incremental {
            let effective_current = applied_versions
                .last()
                .map(|s| s.as_str())
                .unwrap_or(current);

            if compare_versions(migration.version, effective_current) > 0
                && compare_versions(migration.from_version, effective_current) <= 0
            {
                if options.dry_run {
                    applied_versions.push(migration.version.to_string());
                } else {
                    apply_migration(conn, migration, &options)?;
                    applied_versions.push(migration.version.to_string());
                }
            }
        }

        let final_version = applied_versions
            .last()
            .cloned()
            .unwrap_or_else(|| current.to_string());

        Ok(MigrationResult {
            applied_count: applied_versions.len(),
            applied_versions,
            final_version,
            dry_run: options.dry_run,
        })
    })();

    if !options.dry_run {
        match &result {
            Ok(_) => {
                crate::telemetry::info!("committing migration transaction");
                conn.execute_batch("COMMIT")?;
            }
            Err(_e) => {
                crate::telemetry::warn!(
                    error = %crate::telemetry::chain(_e),
                    "rolling back migration transaction"
                );
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }

    result
}

/// Apply a single migration with savepoint.
fn apply_migration(
    conn: &mut Connection,
    migration: &Migration,
    options: &MigrationOptions,
) -> Result<()> {
    crate::telemetry::info!(
        version = migration.version,
        description = migration.description,
        "applying migration"
    );

    let start = std::time::Instant::now();

    let savepoint = conn
        .savepoint()
        .map_err(|e| PoolError::Migration(format!("failed to create savepoint: {}", e)))?;

    if let Err(e) = savepoint.execute_batch(migration.sql) {
        // Savepoint rolls back on drop, so failed migrations aren't recorded.
        return Err(PoolError::Migration(format!(
            "failed to execute migration {}: {}",
            migration.version, e
        )));
    }

    if options.validate_creates && !migration.creates_tables.is_empty() {
        for table in migration.creates_tables {
            let exists: bool = savepoint
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);

            if !exists {
                return Err(PoolError::Migration(format!(
                    "migration {} claims to create table '{}' but it doesn't exist after execution",
                    migration.version, table
                )));
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as i64;

    savepoint
        .execute(
            "INSERT OR REPLACE INTO schema_migrations (version, description, duration_ms, success) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![migration.version, migration.description, duration_ms],
        )
        .map_err(|e| {
            PoolError::Migration(format!(
                "failed to record migration {}: {}",
                migration.version, e
            ))
        })?;

    savepoint
        .execute(
            "INSERT OR REPLACE INTO schema_meta (key, value, updated_at) VALUES ('version', ?1, datetime('now'))",
            rusqlite::params![migration.version],
        )
        .map_err(|e| {
            PoolError::Migration(format!(
                "failed to update version to {}: {}",
                migration.version, e
            ))
        })?;

    savepoint
        .commit()
        .map_err(|e| PoolError::Migration(format!("failed to commit migration: {}", e)))?;

    crate::telemetry::info!(
        version = migration.version,
        duration_ms = duration_ms,
        "migration complete"
    );

    Ok(())
}

/// Get migration history from the database.
///
/// Returns a list of [`MigrationRecord`]s ordered by application time.
///
/// # Example
///
/// ```rust,no_run
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// let history = sqlite_kit::get_migration_history(conn)?;
/// for record in &history {
///     println!("{}: {} (applied: {}, {}ms)",
///         record.version, record.description, record.applied_at, record.duration_ms);
/// }
/// # Ok(())
/// # }
/// ```
pub fn get_migration_history(conn: &Connection) -> Result<Vec<MigrationRecord>> {
    let table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !table_exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT version, COALESCE(description, ''), applied_at, COALESCE(duration_ms, 0), COALESCE(success, 1)
         FROM schema_migrations
         ORDER BY applied_at ASC"
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(MigrationRecord {
            version: row.get(0)?,
            description: row.get(1)?,
            applied_at: row.get(2)?,
            duration_ms: row.get(3)?,
            success: row.get::<_, i64>(4)? != 0,
        })
    })?;

    let mut history = Vec::new();
    for row in rows {
        history.push(row?);
    }

    Ok(history)
}

/// Check if a specific migration has been applied.
///
/// # Example
///
/// ```rust,no_run
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// if sqlite_kit::is_migration_applied(conn, "1.2.0")? {
///     println!("Migration 1.2.0 has been applied");
/// }
/// # Ok(())
/// # }
/// ```
pub fn is_migration_applied(conn: &Connection, version: &str) -> Result<bool> {
    let applied: bool = conn
        .query_row(
            "SELECT 1 FROM schema_migrations WHERE version = ?1 AND success = 1",
            [version],
            |_| Ok(true),
        )
        .unwrap_or(false);

    Ok(applied)
}

/// Get pending migrations that haven't been applied yet.
///
/// This simulates the migration chain to determine all migrations
/// that would be applied, including chained incrementals.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::Migration;
///
/// # fn example(conn: &rusqlite::Connection, migrations: &[Migration]) -> Result<(), sqlite_kit::PoolError> {
/// let pending = sqlite_kit::get_pending_migrations(conn, migrations)?;
/// println!("{} migrations pending", pending.len());
/// # Ok(())
/// # }
/// ```
pub fn get_pending_migrations<'a>(
    conn: &Connection,
    migrations: &'a [Migration],
) -> Result<Vec<&'a Migration>> {
    let current_version: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .ok();

    let mut effective_version = current_version.unwrap_or_else(|| "0.0.0".to_string());
    let mut pending = Vec::new();

    if effective_version == "0.0.0" {
        if let Some(baseline) = migrations
            .iter()
            .find(|m| m.kind == MigrationKind::Baseline)
        {
            pending.push(baseline);
            effective_version = baseline.version.to_string();
        }
    }

    let mut incremental: Vec<_> = migrations
        .iter()
        .filter(|m| m.kind == MigrationKind::Incremental)
        .collect();
    incremental.sort_by(|a, b| compare_versions(a.version, b.version).cmp(&0));

    loop {
        let next = incremental.iter().find(|m| {
            compare_versions(m.version, &effective_version) > 0
                && compare_versions(m.from_version, &effective_version) <= 0
        });

        match next {
            Some(migration) => {
                pending.push(*migration);
                effective_version = migration.version.to_string();
            }
            None => break,
        }
    }

    Ok(pending)
}

#[cfg(test)]
mod tests;
