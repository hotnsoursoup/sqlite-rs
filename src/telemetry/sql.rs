//! SQL statement display wrapper that truncates safely for logs.

use std::borrow::Cow;
use std::fmt;

/// Default maximum length for SQL statements in logs.
pub const DEFAULT_SQL_MAX_LEN: usize = 500;

/// A wrapper for SQL statements that safely truncates for logging.
///
/// Truncates long SQL statements and respects UTF-8 character boundaries.
///
/// # Example
///
/// ```ignore
/// crate::telemetry::info!(
///     db_statement = %SqlStatement::new(sql),
///     "executing query"
/// );
/// ```
#[derive(Debug, Clone)]
pub struct SqlStatement {
    sql: String,
    max_len: usize,
}

impl SqlStatement {
    /// Create a new SQL statement wrapper with default max length (500 chars).
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            max_len: DEFAULT_SQL_MAX_LEN,
        }
    }

    /// Create a new SQL statement wrapper with a custom max length.
    pub fn with_max_len(sql: impl Into<String>, max_len: usize) -> Self {
        Self {
            sql: sql.into(),
            max_len,
        }
    }

    /// Truncate the SQL string safely at UTF-8 boundaries.
    /// The resulting string is guaranteed to be <= max_len bytes.
    fn truncated(&self) -> Cow<'_, str> {
        if self.sql.len() <= self.max_len {
            return Cow::Borrowed(&self.sql);
        }
        // Reserve 3 bytes for the "..." suffix.
        let target_len = self.max_len.saturating_sub(3);

        // Find the last char boundary that keeps us under target_len.
        let mut boundary = 0;
        for (i, c) in self.sql.char_indices() {
            let char_end = i + c.len_utf8();
            if char_end <= target_len {
                boundary = char_end;
            } else {
                break;
            }
        }

        Cow::Owned(format!("{}...", &self.sql[..boundary]))
    }
}

impl fmt::Display for SqlStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.truncated())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_statement_short() {
        let stmt = SqlStatement::new("SELECT 1");
        assert_eq!(stmt.to_string(), "SELECT 1");
    }

    #[test]
    fn test_sql_statement_truncated() {
        let long_sql = "x".repeat(1000);
        let stmt = SqlStatement::with_max_len(long_sql, 100);
        let result = stmt.to_string();
        assert!(result.len() <= 100);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_sql_statement_utf8_safe() {
        // Emoji is 4 bytes
        let emoji_sql = "🔥".repeat(100);
        let stmt = SqlStatement::with_max_len(emoji_sql, 50);
        let result = stmt.to_string();
        assert!(result.len() <= 50);
        for c in result.chars() {
            assert!(c.len_utf8() > 0);
        }
    }
}
