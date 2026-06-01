//! Chunked iteration over large result sets.

#![allow(clippy::type_complexity)]

use crate::error::{PoolError, Result};
use rusqlite::Connection;

/// Chunked iterator for processing large result sets without loading everything into memory.
///
/// Uses LIMIT/OFFSET pagination to fetch rows in chunks.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::batch::ChunkedIterator;
///
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// let iter = ChunkedIterator::<String>::new(
///     conn,
///     "SELECT name FROM users ORDER BY id",
///     100, // chunk size
///     |row| row.get(0),
/// );
///
/// for batch in iter {
///     let names = batch?;
///     println!("Processing {} names", names.len());
/// }
/// # Ok(())
/// # }
/// ```
pub struct ChunkedIterator<'conn, T> {
    conn: &'conn Connection,
    base_query: String,
    chunk_size: usize,
    offset: usize,
    mapper: Box<dyn Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T> + 'conn>,
    exhausted: bool,
}

impl<'conn, T> ChunkedIterator<'conn, T> {
    /// Create a new chunked iterator.
    ///
    /// The query should NOT include LIMIT/OFFSET - these will be added automatically.
    /// For consistent results, the query should include an ORDER BY clause.
    pub fn new<F>(conn: &'conn Connection, query: &str, chunk_size: usize, mapper: F) -> Self
    where
        F: Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T> + 'conn,
    {
        Self {
            conn,
            base_query: query.to_string(),
            chunk_size,
            offset: 0,
            mapper: Box::new(mapper),
            exhausted: false,
        }
    }

    /// Fetch the next chunk of rows.
    fn fetch_chunk(&mut self) -> Result<Vec<T>> {
        let query = format!(
            "{} LIMIT {} OFFSET {}",
            self.base_query, self.chunk_size, self.offset
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| PoolError::Batch(format!("failed to prepare chunked query: {}", e)))?;

        let rows: std::result::Result<Vec<T>, _> = stmt
            .query_map([], |row| (self.mapper)(row))
            .map_err(|e| PoolError::Batch(format!("chunked query failed: {}", e)))?
            .collect();

        let chunk = rows.map_err(|e| PoolError::Batch(format!("row mapping failed: {}", e)))?;

        if chunk.len() < self.chunk_size {
            self.exhausted = true;
        } else {
            self.offset += self.chunk_size;
        }

        Ok(chunk)
    }
}

impl<'conn, T> Iterator for ChunkedIterator<'conn, T> {
    type Item = Result<Vec<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }

        let chunk = self.fetch_chunk();

        match &chunk {
            Ok(c) if c.is_empty() => None,
            _ => Some(chunk),
        }
    }
}

/// Process a query in chunks, calling a callback for each chunk.
///
/// This is useful for processing large result sets without loading
/// everything into memory at once.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::batch::chunk_query;
///
/// # fn example(conn: &rusqlite::Connection) -> Result<(), sqlite_kit::PoolError> {
/// let mut total = 0;
///
/// chunk_query(
///     conn,
///     "SELECT id, name FROM users ORDER BY id",
///     1000,
///     |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
///     |chunk: Vec<(i64, String)>| {
///         total += chunk.len();
///         println!("Processed {} users so far", total);
///         Ok(()) // Return Ok to continue, Err to stop
///     },
/// )?;
/// # Ok(())
/// # }
/// ```
pub fn chunk_query<T, F, C>(
    conn: &Connection,
    query: &str,
    chunk_size: usize,
    mapper: F,
    mut callback: C,
) -> Result<usize>
where
    F: Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    C: FnMut(Vec<T>) -> Result<()>,
{
    let mut offset = 0;
    let mut total_processed = 0;

    loop {
        let paginated_query = format!("{} LIMIT {} OFFSET {}", query, chunk_size, offset);

        let mut stmt = conn
            .prepare(&paginated_query)
            .map_err(|e| PoolError::Batch(format!("failed to prepare query: {}", e)))?;

        let rows: std::result::Result<Vec<T>, _> = stmt
            .query_map([], |row| mapper(row))
            .map_err(|e| PoolError::Batch(format!("query failed: {}", e)))?
            .collect();

        let chunk = rows.map_err(|e| PoolError::Batch(format!("row mapping failed: {}", e)))?;
        let chunk_len = chunk.len();

        if chunk_len == 0 {
            break;
        }

        total_processed += chunk_len;
        callback(chunk)?;

        if chunk_len < chunk_size {
            break;
        }

        offset += chunk_size;
    }

    Ok(total_processed)
}

/// Count the total number of rows that would be returned by a query.
///
/// Wraps the query in a COUNT(*) subquery.
pub fn count_query(conn: &Connection, query: &str) -> Result<i64> {
    let count_query = format!("SELECT COUNT(*) FROM ({})", query);

    conn.query_row(&count_query, [], |row| row.get(0))
        .map_err(|e| PoolError::Batch(format!("count query failed: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunked_iterator() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE data (id INTEGER PRIMARY KEY, value TEXT)", [])
            .unwrap();

        // Insert test data
        for i in 0..25 {
            conn.execute(
                "INSERT INTO data (value) VALUES (?)",
                [format!("value_{}", i)],
            )
            .unwrap();
        }

        // Iterate in chunks of 10
        let iter = ChunkedIterator::<String>::new(
            &conn,
            "SELECT value FROM data ORDER BY id",
            10,
            |row| row.get(0),
        );

        let mut total = 0;
        let mut chunk_count = 0;

        for batch in iter {
            let values = batch.unwrap();
            total += values.len();
            chunk_count += 1;
        }

        assert_eq!(total, 25);
        assert_eq!(chunk_count, 3); // 10 + 10 + 5
    }

    #[test]
    fn test_chunk_query() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE data (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        for _ in 0..15 {
            conn.execute("INSERT INTO data DEFAULT VALUES", []).unwrap();
        }

        let mut chunks_received = 0;

        let total = chunk_query(
            &conn,
            "SELECT id FROM data ORDER BY id",
            5,
            |row| row.get::<_, i64>(0),
            |chunk: Vec<i64>| {
                chunks_received += 1;
                assert!(chunk.len() <= 5);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(total, 15);
        assert_eq!(chunks_received, 3);
    }

    #[test]
    fn test_count_query() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE data (id INTEGER PRIMARY KEY, active BOOLEAN)",
            [],
        )
        .unwrap();

        for i in 0..20 {
            conn.execute("INSERT INTO data (active) VALUES (?)", [i % 2 == 0])
                .unwrap();
        }

        let count = count_query(&conn, "SELECT * FROM data WHERE active = 1").unwrap();
        assert_eq!(count, 10);
    }
}
