//! Migration framework with semantic versioning support.
//!
//! This module provides a robust migration system for SQLite databases:
//!
//! - **Baseline migrations**: Applied to fresh databases
//! - **Incremental migrations**: Applied in order based on version
//! - **Savepoint rollback**: Failed migrations are rolled back atomically
//! - **Version tracking**: Migrations are recorded in `schema_migrations` table
//! - **Dry-run mode**: Preview migrations without applying them
//! - **Schema validation**: Verify `creates_tables` after each migration
//! - **Database recovery**: Reset or repair corrupted databases
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_rs::{
//!     DatabasePool, Migration, MigrationKind, MigrationOptions,
//!     run_migrations, run_migrations_with_options,
//!     get_migration_history, get_pending_migrations,
//!     validate_schema_integrity, reset_database,
//! };
//!
//! const MIGRATIONS: &[Migration] = &[
//!     Migration {
//!         version: "1.0.0",
//!         from_version: "0.0.0",
//!         description: "Initial schema",
//!         sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
//!         kind: MigrationKind::Baseline,
//!         creates_tables: &["users"],
//!     },
//!     Migration {
//!         version: "1.1.0",
//!         from_version: "1.0.0",
//!         description: "Add email column",
//!         sql: "ALTER TABLE users ADD COLUMN email TEXT;",
//!         kind: MigrationKind::Incremental,
//!         creates_tables: &[],
//!     },
//! ];
//!
//! # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_rs::PoolError> {
//! // Simple migration (recommended for most cases)
//! run_migrations(conn, MIGRATIONS)?;
//!
//! // Or with options (dry-run, app name, etc.)
//! let result = run_migrations_with_options(
//!     conn,
//!     MIGRATIONS,
//!     MigrationOptions::new()
//!         .with_app_name("my_app")
//!         .dry_run()
//! )?;
//! println!("Would apply {} migrations", result.applied_count);
//! # Ok(())
//! # }
//! ```
//!
//! # Migration Builder API
//!
//! For cleaner migration definitions:
//!
//! ```rust
//! use sqlite_rs::Migration;
//!
//! const MIGRATIONS: &[Migration] = &[
//!     Migration::baseline("1.0.0", r#"
//!         CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
//!         CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER);
//!     "#).creates(&["users", "posts"]),
//!
//!     Migration::incremental("1.1.0", "1.0.0", "ALTER TABLE users ADD COLUMN email TEXT;")
//!         .with_description("Add email to users"),
//! ];
//! ```
//!
//! # Schema Validation and Recovery
//!
//! ```rust,no_run
//! use sqlite_rs::{validate_schema_integrity, reset_database, Migration};
//!
//! # fn example(conn: &mut rusqlite::Connection, migrations: &[Migration]) -> Result<(), sqlite_rs::PoolError> {
//! // Check schema integrity
//! let issues = validate_schema_integrity(conn, &["users", "posts"])?;
//! if !issues.is_empty() {
//!     println!("Schema issues: {:?}", issues);
//!     // Reset if corrupted
//!     reset_database(conn, migrations)?;
//! }
//! # Ok(())
//! # }
//! ```

mod recovery;
mod runner;
mod types;
mod version;

// Core types
pub use types::{Migration, MigrationKind};

// Runner functions and types
pub use runner::{
    get_migration_history, get_pending_migrations, is_migration_applied, run_migrations,
    run_migrations_with_options, MigrationOptions, MigrationRecord, MigrationResult,
};

// Version functions and types
pub use version::{compare_versions, detect_schema_state, get_current_version, SchemaState};

// Recovery functions and types
pub use recovery::{
    drop_all_tables, repair_database, reset_database, reset_database_with_options,
    validate_schema_integrity, validate_table_columns, RepairResult, SchemaIssue,
};
