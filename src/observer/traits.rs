//! Core observer trait and error types.

use super::context::OperationContext;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Errors that can occur in observers.
#[derive(Debug, Error)]
pub enum ObserverError {
    /// Operation was blocked by an observer.
    #[error("operation blocked: {reason}")]
    Blocked { reason: String },

    /// Rate limit exceeded.
    #[error("rate limit exceeded: {limit} requests per {window_secs}s")]
    RateLimitExceeded { limit: u32, window_secs: u64 },

    /// Generic observer error.
    #[error("observer error: {0}")]
    Other(String),
}

/// Result type for observer operations.
pub type ObserverResult<T> = std::result::Result<T, ObserverError>;

/// Observer trait for database operation lifecycle hooks.
///
/// Implement this trait to observe or block database operations. Observers are
/// attached directly to [`PoolConfig`](crate::PoolConfig), so the main
/// [`DatabasePool`](crate::DatabasePool) surface is the single composition root
/// for profiling, logging, metrics, and policy checks.
///
/// # Lifecycle
///
/// 1. `before_operation` runs before an operation starts and may reject it.
/// 2. The operation executes if all observers allow it.
/// 3. `after_operation` runs in reverse stack order with timing and outcome.
///
/// # Example
///
/// ```rust
/// use sqlite_kit::observer::{Observer, ObserverResult, OperationContext};
/// use std::time::Duration;
///
/// struct MyObserver;
///
/// impl Observer for MyObserver {
///     fn before_operation(&self, ctx: &mut OperationContext) -> ObserverResult<()> {
///         println!("about to execute: {}", ctx.sql());
///         Ok(())
///     }
///
///     fn after_operation(&self, ctx: &OperationContext, duration: Duration, success: bool) {
///         println!("{} took {:?}; success={}", ctx.sql(), duration, success);
///     }
/// }
/// ```
pub trait Observer: Send + Sync {
    /// Called before an operation executes.
    ///
    /// Return `Ok(())` to proceed, or an [`ObserverError`] to reject the
    /// operation before it touches SQLite.
    fn before_operation(&self, _ctx: &mut OperationContext) -> ObserverResult<()> {
        Ok(())
    }

    /// Called after an operation completes successfully or with an error.
    ///
    /// This hook is observational and cannot fail.
    fn after_operation(&self, _ctx: &OperationContext, _duration: Duration, _success: bool) {}

    /// Human-readable observer name for diagnostics.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl Observer for Arc<dyn Observer> {
    fn before_operation(&self, ctx: &mut OperationContext) -> ObserverResult<()> {
        (**self).before_operation(ctx)
    }

    fn after_operation(&self, ctx: &OperationContext, duration: Duration, success: bool) {
        (**self).after_operation(ctx, duration, success)
    }

    fn name(&self) -> &'static str {
        (**self).name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestObserver {
        should_block: bool,
    }

    impl Observer for TestObserver {
        fn before_operation(&self, _ctx: &mut OperationContext) -> ObserverResult<()> {
            if self.should_block {
                Err(ObserverError::Blocked {
                    reason: "test block".to_string(),
                })
            } else {
                Ok(())
            }
        }

        fn name(&self) -> &'static str {
            "TestObserver"
        }
    }

    #[test]
    fn test_observer_allow() {
        let observer = TestObserver {
            should_block: false,
        };
        let mut ctx = OperationContext::new("SELECT 1");
        assert!(observer.before_operation(&mut ctx).is_ok());
    }

    #[test]
    fn test_observer_block() {
        let observer = TestObserver { should_block: true };
        let mut ctx = OperationContext::new("SELECT 1");
        let result = observer.before_operation(&mut ctx);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ObserverError::Blocked { .. }));
    }

    #[test]
    fn test_observer_name() {
        let observer = TestObserver {
            should_block: false,
        };
        assert_eq!(observer.name(), "TestObserver");
    }
}
