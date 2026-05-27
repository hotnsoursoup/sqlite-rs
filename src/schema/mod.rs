//! Schema introspection and safe modification utilities.
//!
//! This module provides utilities for:
//! - Checking if tables/columns/indexes exist
//! - Safely adding columns with graceful handling of "already exists"
//! - Schema introspection for runtime adaptation
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_rs::schema::{column_exists, safe_add_column, ColumnDef};
//!
//! # fn example(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
//! // Check if column exists before querying it
//! if column_exists(conn, "users", "email")? {
//!     // Safe to query email column
//! }
//!
//! // Add column if it doesn't exist (idempotent)
//! safe_add_column(conn, "users", ColumnDef::new("status", "TEXT").default("'active'"))?;
//! # Ok(())
//! # }
//! ```

mod alter;
mod introspection;

pub use introspection::{
    column_exists, get_full_table_info, get_index_info, get_table_columns, get_table_info,
    index_exists, list_tables, table_exists, ColumnInfo, IndexInfo, TableInfo,
};

pub use alter::{safe_add_column, safe_create_index, safe_drop_column, ColumnDef, IndexDef};
