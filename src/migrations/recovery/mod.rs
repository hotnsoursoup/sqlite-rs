//! Database recovery and reset functionality.
//!
//! Three layers, increasing in destructiveness:
//!
//! 1. **Validate** ([`validate_schema_integrity`], [`validate_table_columns`]) —
//!    read-only checks; report missing tables/columns and FK/integrity issues.
//! 2. **Reset** ([`reset_database`], [`reset_database_with_options`],
//!    [`drop_all_tables`]) — drop all user tables then re-run migrations.
//! 3. **Repair** ([`repair_database`]) — drop only tables that fail a trivial
//!    sanity query and re-run migrations to rebuild them. Falls back to a
//!    full reset when nothing is salvageable.
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_kit::{validate_schema_integrity, reset_database, Migration};
//!
//! # fn example(conn: &mut rusqlite::Connection, migrations: &[Migration]) -> Result<(), sqlite_kit::PoolError> {
//! // Check if schema is valid
//! let issues = validate_schema_integrity(conn, &["users", "posts"])?;
//! if !issues.is_empty() {
//!     println!("Schema issues found: {:?}", issues);
//!     // Reset database if corrupted
//!     reset_database(conn, migrations)?;
//! }
//! # Ok(())
//! # }
//! ```

mod repair;
mod reset;
mod validate;

pub use repair::{repair_database, RepairResult};
pub use reset::{drop_all_tables, reset_database, reset_database_with_options};
pub use validate::{validate_schema_integrity, validate_table_columns, SchemaIssue};

#[cfg(test)]
mod tests;
