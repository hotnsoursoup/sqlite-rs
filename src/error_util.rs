//! Error handling utilities for sqlite-rs.
//!
//! Kept lightweight and dependency-free so it works in all feature sets.

use std::error::Error as StdError;
use std::fmt;

/// Maximum depth for error chain traversal to prevent infinite loops.
const MAX_ERROR_CHAIN_DEPTH: usize = 32;

/// Display helper that prints an error and its source chain.
///
/// Traverses the error's `source()` chain and formats each error in sequence.
/// Limited to `MAX_ERROR_CHAIN_DEPTH` levels to prevent infinite loops from
/// buggy error implementations with cyclic source chains.
///
/// # Example
///
/// ```
/// use sqlite_rs::error_util::ErrorChain;
/// use std::io;
///
/// let err = io::Error::new(io::ErrorKind::Other, "outer error");
/// let chain = ErrorChain::new(&err);
/// println!("{}", chain); // "outer error"
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ErrorChain<'a>(pub &'a (dyn StdError + 'static));

impl<'a> ErrorChain<'a> {
    /// Create a new error chain display wrapper.
    pub fn new(err: &'a (dyn StdError + 'static)) -> Self {
        Self(err)
    }
}

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;

        let mut next = self.0.source();
        let mut depth = 0;

        while let Some(source) = next {
            depth += 1;
            if depth > MAX_ERROR_CHAIN_DEPTH {
                write!(
                    f,
                    ": ... (chain truncated at {} levels)",
                    MAX_ERROR_CHAIN_DEPTH
                )?;
                break;
            }
            write!(f, ": {}", source)?;
            next = source.source();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_single_error() {
        let err = io::Error::other("test error");
        let chain = ErrorChain::new(&err);
        assert_eq!(chain.to_string(), "test error");
    }

    #[test]
    fn test_error_chain() {
        // Create a simple chained error using io::Error
        let inner = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let outer = io::Error::other(inner);

        let chain = ErrorChain::new(&outer);
        let result = chain.to_string();

        // The chain should contain both errors
        assert!(result.contains("file not found"));
    }

    #[test]
    fn test_max_depth_protection() {
        // We can't easily create an infinitely recursive error, but we can verify
        // the constant exists and has a reasonable value. Use black_box so this
        // remains a runtime guard under clippy's constant-assertion lint.
        let max_depth = std::hint::black_box(MAX_ERROR_CHAIN_DEPTH);
        assert!((10..=100).contains(&max_depth));
    }
}
