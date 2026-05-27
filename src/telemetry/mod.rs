//! Logging / tracing helpers for sqlite-rs.
//!
//! All macros are compiled out unless the `tracing` feature is enabled.
//!
//! # Usage Guidelines
//!
//! - Use `crate::telemetry::*` macros instead of `tracing::*` directly
//! - Use semantic field names: `db_system`, `db_operation`, `db_statement`
//! - Never log secrets - use [`RedactedConnectionString`] for connection strings
//! - Include error chains: `error = %crate::telemetry::chain(&err)`
//! - Truncate SQL safely: `db_statement = %SqlStatement::new(sql)`
//! - For spans use [`DbSpan`] (no-op unless `tracing` feature is enabled)

mod macros;
mod redact;
mod span;
mod sql;

pub use redact::RedactedConnectionString;
pub use span::DbSpan;
#[cfg(not(feature = "tracing"))]
pub use span::DbSpanGuard;
pub use sql::{SqlStatement, DEFAULT_SQL_MAX_LEN};

// Re-export the user-facing macro names (the `__sqlite_rs_*` names are
// crate-root macros declared with `#[macro_export]` so the aliases work
// transparently from any module).
pub use crate::__sqlite_rs_debug as debug;
pub use crate::__sqlite_rs_error as error;
pub use crate::__sqlite_rs_info as info;
pub use crate::__sqlite_rs_trace as trace;
pub use crate::__sqlite_rs_warn as warn;

use crate::error_util::ErrorChain;

/// Get a display wrapper for an error and its source chain.
///
/// # Example
///
/// ```ignore
/// crate::telemetry::error!(
///     error = %crate::telemetry::chain(&err),
///     "operation failed"
/// );
/// ```
#[inline]
pub fn chain<'a>(err: &'a (dyn std::error::Error + 'static)) -> ErrorChain<'a> {
    ErrorChain::new(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_display() {
        let err = std::io::Error::other("test error");
        let chained = chain(&err);
        assert_eq!(chained.to_string(), "test error");
    }
}
