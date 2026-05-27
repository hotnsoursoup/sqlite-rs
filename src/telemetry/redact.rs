//! Redact secrets from connection strings before they hit the log.

use std::fmt;

/// A wrapper for connection strings that redacts sensitive information.
///
/// Hides passwords and tokens from connection strings before logging.
///
/// # Example
///
/// ```ignore
/// crate::telemetry::info!(
///     connection = %RedactedConnectionString::new(url),
///     "connecting to database"
/// );
/// ```
#[derive(Debug, Clone)]
pub struct RedactedConnectionString {
    original: String,
}

impl RedactedConnectionString {
    /// Create a new redacted connection string.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            original: url.into(),
        }
    }

    /// Redact sensitive parts of the connection string.
    fn redacted(&self) -> String {
        let mut result = self.original.clone();

        // URL-style credentials: `user:password@host` → `user:***@host`.
        if let Some(at_pos) = result.find('@') {
            if let Some(colon_pos) = result[..at_pos].rfind(':') {
                let scheme_end = result.find("://").map(|p| p + 3).unwrap_or(0);
                if colon_pos > scheme_end {
                    result = format!("{}:***@{}", &result[..colon_pos], &result[at_pos + 1..]);
                }
            }
        }

        // Query parameter secrets.
        for param in &[
            "password", "pwd", "token", "api_key", "apikey", "secret", "key",
        ] {
            let patterns = [format!("{}=", param), format!("{}_=", param)];
            for pattern in &patterns {
                if let Some(pos) = result.to_lowercase().find(&pattern.to_lowercase()) {
                    let value_start = pos + pattern.len();
                    let value_end = result[value_start..]
                        .find(['&', ' ', '#'])
                        .map(|p| value_start + p)
                        .unwrap_or(result.len());
                    result = format!(
                        "{}{}***{}",
                        &result[..value_start],
                        "",
                        &result[value_end..]
                    );
                }
            }
        }

        result
    }
}

impl fmt::Display for RedactedConnectionString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.redacted())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redacted_connection_string_simple() {
        let url = "sqlite:///path/to/db.sqlite";
        let redacted = RedactedConnectionString::new(url);
        assert_eq!(redacted.to_string(), url);
    }

    #[test]
    fn test_redacted_connection_string_with_password() {
        let url = "postgresql://user:secret123@localhost/db";
        let redacted = RedactedConnectionString::new(url);
        assert!(redacted.to_string().contains("***"));
        assert!(!redacted.to_string().contains("secret123"));
    }

    #[test]
    fn test_redacted_connection_string_query_params() {
        let url = "sqlite:///db.sqlite?password=secret&other=value";
        let redacted = RedactedConnectionString::new(url);
        let result = redacted.to_string();
        assert!(result.contains("***"));
        assert!(!result.contains("secret"));
        assert!(result.contains("other=value"));
    }
}
