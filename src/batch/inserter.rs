//! Batch insert operations.

use crate::error::{PoolError, Result};
use crate::sql::{quote_identifier, quote_qualified_identifier};
use rusqlite::types::{ToSql, Value};
use rusqlite::Connection;

/// Configuration for batch operations.
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum number of rows per INSERT statement.
    /// SQLite has a limit on the number of terms in a compound SELECT (default 500).
    pub rows_per_statement: usize,

    /// Whether to use a transaction for the entire batch.
    pub use_transaction: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            rows_per_statement: 100,
            use_transaction: true,
        }
    }
}

impl BatchConfig {
    /// Create a config for small batches (good for memory-constrained environments).
    pub fn small() -> Self {
        Self {
            rows_per_statement: 25,
            use_transaction: true,
        }
    }

    /// Create a config for large batches (good for bulk imports).
    pub fn large() -> Self {
        Self {
            rows_per_statement: 500,
            use_transaction: true,
        }
    }
}

/// Mode for handling conflicts during insert.
#[derive(Debug, Clone, Copy, Default)]
pub enum UpsertMode {
    /// Regular insert - fails on conflict.
    #[default]
    Insert,
    /// Replace existing row on conflict.
    Replace,
    /// Ignore row on conflict.
    Ignore,
}

impl UpsertMode {
    fn sql_prefix(&self) -> &'static str {
        match self {
            UpsertMode::Insert => "INSERT INTO",
            UpsertMode::Replace => "INSERT OR REPLACE INTO",
            UpsertMode::Ignore => "INSERT OR IGNORE INTO",
        }
    }
}

fn validate_batch_config(config: &BatchConfig) -> Result<()> {
    if config.rows_per_statement == 0 {
        return Err(PoolError::Batch(
            "rows_per_statement must be greater than zero".to_string(),
        ));
    }

    Ok(())
}

fn validate_chunk_shape<T>(columns: &[String], chunk: &[Vec<T>]) -> Result<()> {
    if columns.is_empty() {
        return Err(PoolError::Batch(
            "batch insert requires at least one column".to_string(),
        ));
    }

    for (row_index, row) in chunk.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(PoolError::Batch(format!(
                "batch row {} has {} value(s), expected {}",
                row_index,
                row.len(),
                columns.len()
            )));
        }
    }

    Ok(())
}

fn build_insert_sql(
    upsert_mode: UpsertMode,
    table: &str,
    columns: &[String],
    row_count: usize,
) -> Result<String> {
    let table = quote_qualified_identifier(table).ok_or_else(|| {
        PoolError::Batch("table identifier must not be empty or malformed".to_string())
    })?;

    let columns_str = columns
        .iter()
        .map(|column| {
            let column = column.trim();
            if column.is_empty() {
                Err(PoolError::Batch(
                    "column identifiers must not be empty".to_string(),
                ))
            } else {
                Ok(quote_identifier(column))
            }
        })
        .collect::<Result<Vec<_>>>()?
        .join(", ");

    let placeholders_per_row = (0..columns.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let all_placeholders = (0..row_count)
        .map(|_| format!("({})", placeholders_per_row))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!(
        "{} {} ({}) VALUES {}",
        upsert_mode.sql_prefix(),
        table,
        columns_str,
        all_placeholders
    ))
}

/// A batch inserter for efficiently inserting many rows.
///
/// Collects rows and executes them in batches using multi-row INSERT statements.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::batch::{BatchInserter, UpsertMode};
///
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// # conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)", [])?;
/// let mut inserter = BatchInserter::new("users", &["name", "email"])
///     .with_upsert_mode(UpsertMode::Replace);
///
/// inserter.add(&["Alice", "alice@example.com"]);
/// inserter.add(&["Bob", "bob@example.com"]);
///
/// let count = inserter.execute(conn)?;
/// assert_eq!(count, 2);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct BatchInserter {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    config: BatchConfig,
    upsert_mode: UpsertMode,
}

impl BatchInserter {
    /// Create a new batch inserter for the given table and columns.
    pub fn new(table: impl Into<String>, columns: &[&str]) -> Self {
        Self {
            table: table.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows: Vec::new(),
            config: BatchConfig::default(),
            upsert_mode: UpsertMode::default(),
        }
    }

    /// Set the batch configuration.
    pub fn with_config(mut self, config: BatchConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the upsert mode.
    pub fn with_upsert_mode(mut self, mode: UpsertMode) -> Self {
        self.upsert_mode = mode;
        self
    }

    /// Add a row to the batch.
    ///
    /// Values are converted to strings and bound as query parameters.
    pub fn add<S: AsRef<str>>(&mut self, values: &[S]) {
        let row: Vec<String> = values.iter().map(|s| s.as_ref().to_string()).collect();
        self.rows.push(row);
    }

    /// Add a row from an iterator of string-like values.
    pub fn add_iter<I, S>(&mut self, values: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let row: Vec<String> = values.into_iter().map(|s| s.as_ref().to_string()).collect();
        self.rows.push(row);
    }

    /// Get the number of rows pending insertion.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Check if there are no pending rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Clear all pending rows without inserting.
    pub fn clear(&mut self) {
        self.rows.clear();
    }

    /// Execute the batch insert, returning the number of rows inserted.
    pub fn execute(&mut self, conn: &mut Connection) -> Result<usize> {
        if self.rows.is_empty() {
            return Ok(0);
        }

        validate_batch_config(&self.config)?;

        let mut total_inserted = 0;

        if self.config.use_transaction {
            let tx = conn
                .transaction()
                .map_err(|e| PoolError::Batch(format!("failed to start transaction: {}", e)))?;

            for chunk in self.rows.chunks(self.config.rows_per_statement) {
                total_inserted += self.execute_chunk(&tx, chunk)?;
            }

            tx.commit()
                .map_err(|e| PoolError::Batch(format!("failed to commit batch: {}", e)))?;
        } else {
            for chunk in self.rows.chunks(self.config.rows_per_statement) {
                total_inserted += self.execute_chunk(conn, chunk)?;
            }
        }

        self.rows.clear();
        Ok(total_inserted)
    }

    /// Execute a single chunk of rows.
    fn execute_chunk(&self, conn: &Connection, chunk: &[Vec<String>]) -> Result<usize> {
        if chunk.is_empty() {
            return Ok(0);
        }

        validate_chunk_shape(&self.columns, chunk)?;

        // Build multi-row INSERT statement
        let sql = build_insert_sql(self.upsert_mode, &self.table, &self.columns, chunk.len())?;

        // Flatten all values into a single params vector
        let params: Vec<&dyn rusqlite::ToSql> = chunk
            .iter()
            .flat_map(|row| row.iter().map(|s| s as &dyn rusqlite::ToSql))
            .collect();

        let inserted = conn
            .execute(&sql, params.as_slice())
            .map_err(|e| PoolError::Batch(format!("batch insert failed: {}", e)))?;

        Ok(inserted)
    }
}

/// A typed batch inserter that supports any `ToSql` types.
///
/// Unlike `BatchInserter` which only handles strings, this inserter supports
/// integers, floats, blobs, NULL values, and any type implementing `ToSql`.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::batch::{TypedBatchInserter, BatchConfig, UpsertMode};
/// use rusqlite::types::Value;
///
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// # conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price REAL, stock INTEGER)", [])?;
/// let mut inserter = TypedBatchInserter::new("products", &["name", "price", "stock"]);
///
/// // Add rows with mixed types
/// inserter.add_row(&[
///     Value::Text("Widget".to_string()),
///     Value::Real(29.99),
///     Value::Integer(100),
/// ]);
///
/// inserter.add_row(&[
///     Value::Text("Gadget".to_string()),
///     Value::Real(49.99),
///     Value::Integer(50),
/// ]);
///
/// // Or use the builder pattern for cleaner code
/// inserter.row()
///     .text("Gizmo")
///     .real(19.99)
///     .integer(200)
///     .finish();
///
/// let count = inserter.execute(conn)?;
/// assert_eq!(count, 3);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct TypedBatchInserter {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    config: BatchConfig,
    upsert_mode: UpsertMode,
}

impl TypedBatchInserter {
    /// Create a new typed batch inserter for the given table and columns.
    pub fn new(table: impl Into<String>, columns: &[&str]) -> Self {
        Self {
            table: table.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows: Vec::new(),
            config: BatchConfig::default(),
            upsert_mode: UpsertMode::default(),
        }
    }

    /// Set the batch configuration.
    pub fn with_config(mut self, config: BatchConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the upsert mode.
    pub fn with_upsert_mode(mut self, mode: UpsertMode) -> Self {
        self.upsert_mode = mode;
        self
    }

    /// Add a row with typed values.
    ///
    /// Values are stored as `rusqlite::types::Value` which supports:
    /// - `Value::Null` - NULL value
    /// - `Value::Integer(i64)` - integers
    /// - `Value::Real(f64)` - floating point
    /// - `Value::Text(String)` - text
    /// - `Value::Blob(Vec<u8>)` - binary data
    pub fn add_row(&mut self, values: &[Value]) {
        self.rows.push(values.to_vec());
    }

    /// Start building a row with the fluent API.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlite_kit::batch::TypedBatchInserter;
    /// # let mut inserter = TypedBatchInserter::new("test", &["a", "b", "c"]);
    /// inserter.row()
    ///     .text("hello")
    ///     .integer(42)
    ///     .null()
    ///     .finish();
    /// ```
    pub fn row(&mut self) -> RowBuilder<'_> {
        RowBuilder {
            inserter: self,
            values: Vec::new(),
        }
    }

    /// Get the number of rows pending insertion.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Check if there are no pending rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Clear all pending rows without inserting.
    pub fn clear(&mut self) {
        self.rows.clear();
    }

    /// Execute the batch insert, returning the number of rows inserted.
    pub fn execute(&mut self, conn: &mut Connection) -> Result<usize> {
        if self.rows.is_empty() {
            return Ok(0);
        }

        validate_batch_config(&self.config)?;

        let mut total_inserted = 0;

        if self.config.use_transaction {
            let tx = conn
                .transaction()
                .map_err(|e| PoolError::Batch(format!("failed to start transaction: {}", e)))?;

            for chunk in self.rows.chunks(self.config.rows_per_statement) {
                total_inserted += self.execute_chunk(&tx, chunk)?;
            }

            tx.commit()
                .map_err(|e| PoolError::Batch(format!("failed to commit batch: {}", e)))?;
        } else {
            for chunk in self.rows.chunks(self.config.rows_per_statement) {
                total_inserted += self.execute_chunk(conn, chunk)?;
            }
        }

        self.rows.clear();
        Ok(total_inserted)
    }

    /// Execute a single chunk of rows.
    fn execute_chunk(&self, conn: &Connection, chunk: &[Vec<Value>]) -> Result<usize> {
        if chunk.is_empty() {
            return Ok(0);
        }

        validate_chunk_shape(&self.columns, chunk)?;

        // Build multi-row INSERT statement
        let sql = build_insert_sql(self.upsert_mode, &self.table, &self.columns, chunk.len())?;

        // Flatten all values into a single params vector
        let params: Vec<&dyn ToSql> = chunk
            .iter()
            .flat_map(|row| row.iter().map(|v| v as &dyn ToSql))
            .collect();

        let inserted = conn
            .execute(&sql, params.as_slice())
            .map_err(|e| PoolError::Batch(format!("batch insert failed: {}", e)))?;

        Ok(inserted)
    }
}

/// Builder for adding a row with fluent API.
#[derive(Debug)]
pub struct RowBuilder<'a> {
    inserter: &'a mut TypedBatchInserter,
    values: Vec<Value>,
}

impl<'a> RowBuilder<'a> {
    /// Add a text value.
    pub fn text(mut self, value: impl Into<String>) -> Self {
        self.values.push(Value::Text(value.into()));
        self
    }

    /// Add an integer value.
    pub fn integer(mut self, value: i64) -> Self {
        self.values.push(Value::Integer(value));
        self
    }

    /// Add a real (floating point) value.
    pub fn real(mut self, value: f64) -> Self {
        self.values.push(Value::Real(value));
        self
    }

    /// Add a blob (binary) value.
    pub fn blob(mut self, value: impl Into<Vec<u8>>) -> Self {
        self.values.push(Value::Blob(value.into()));
        self
    }

    /// Add a NULL value.
    pub fn null(mut self) -> Self {
        self.values.push(Value::Null);
        self
    }

    /// Add any value that implements `Into<Value>`.
    pub fn value(mut self, value: impl Into<Value>) -> Self {
        self.values.push(value.into());
        self
    }

    /// Finish building the row and add it to the inserter.
    pub fn finish(self) {
        self.inserter.rows.push(self.values);
    }
}

/// Execute a single multi-row insert with the given values.
///
/// This is a convenience function for one-off batch inserts.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::batch::insert_many;
///
/// # fn example(conn: &mut rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// let rows = vec![
///     vec!["Alice", "alice@example.com"],
///     vec!["Bob", "bob@example.com"],
/// ];
///
/// let count = insert_many(conn, "users", &["name", "email"], &rows)?;
/// # Ok(())
/// # }
/// ```
pub fn insert_many<S: AsRef<str>>(
    conn: &mut Connection,
    table: &str,
    columns: &[&str],
    rows: &[Vec<S>],
) -> Result<usize> {
    let mut inserter = BatchInserter::new(table, columns);
    for row in rows {
        let values: Vec<&str> = row.iter().map(|s| s.as_ref()).collect();
        inserter.add(&values);
    }
    inserter.execute(conn)
}

#[cfg(test)]
mod tests;
