//! Batch operations for efficient bulk data handling.
//!
//! Provides utilities for:
//! - Bulk inserts with automatic batching
//! - Chunked iteration over large result sets
//! - Upsert (INSERT OR REPLACE) operations
//! - Typed batch inserts with mixed column types
//!
//! # Example
//!
//! ```rust,no_run
//! use sqlite_kit::batch::{BatchInserter, TypedBatchInserter, BatchConfig};
//!
//! # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
//! // String-only bulk insert
//! let mut inserter = BatchInserter::new("users", &["name", "email"]);
//! inserter.add(&["Alice", "alice@example.com"]);
//! inserter.add(&["Bob", "bob@example.com"]);
//! let inserted = inserter.execute(conn)?;
//!
//! // Typed bulk insert with mixed types (integers, floats, etc.)
//! # conn.execute("CREATE TABLE products (name TEXT, price REAL, stock INTEGER)", [])?;
//! let mut typed = TypedBatchInserter::new("products", &["name", "price", "stock"]);
//! typed.row().text("Widget").real(29.99).integer(100).finish();
//! typed.row().text("Gadget").real(49.99).integer(50).finish();
//! let inserted = typed.execute(conn)?;
//! # Ok(())
//! # }
//! ```

mod inserter;
mod iterator;

pub use inserter::{
    insert_many, BatchConfig, BatchInserter, RowBuilder, TypedBatchInserter, UpsertMode,
};
pub use iterator::{chunk_query, count_query, ChunkedIterator};
