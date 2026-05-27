//! Observer stack for composing operation observers.

use super::context::OperationContext;
use super::traits::{Observer, ObserverResult};
use std::sync::Arc;
use std::time::Duration;

/// Ordered collection of database operation observers.
///
/// # Execution Order
///
/// - `before_operation`: insertion order, first added runs first.
/// - `after_operation`: reverse insertion order, last added runs first.
#[derive(Clone, Default)]
pub struct ObserverStack {
    observers: Vec<Arc<dyn Observer>>,
}

impl std::fmt::Debug for ObserverStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObserverStack")
            .field("observers", &self.names())
            .finish()
    }
}

impl ObserverStack {
    /// Create an empty observer stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an observer to the stack.
    pub fn push<O: Observer + 'static>(&mut self, observer: O) {
        self.observers.push(Arc::new(observer));
    }

    /// Add a shared observer to the stack.
    pub fn push_arc(&mut self, observer: Arc<dyn Observer>) {
        self.observers.push(observer);
    }

    /// Number of observers in the stack.
    pub fn len(&self) -> usize {
        self.observers.len()
    }

    /// Whether the stack has no observers.
    pub fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }

    /// Run all pre-operation hooks in insertion order.
    pub fn before_operation(&self, ctx: &mut OperationContext) -> ObserverResult<()> {
        for observer in &self.observers {
            observer.before_operation(ctx)?;
        }
        Ok(())
    }

    /// Run all post-operation hooks in reverse insertion order.
    pub fn after_operation(&self, ctx: &OperationContext, duration: Duration, success: bool) {
        for observer in self.observers.iter().rev() {
            observer.after_operation(ctx, duration, success);
        }
    }

    /// Names of all observers in insertion order.
    pub fn names(&self) -> Vec<&'static str> {
        self.observers
            .iter()
            .map(|observer| observer.name())
            .collect()
    }

    /// Remove all observers.
    pub fn clear(&mut self) {
        self.observers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct OrderTracker {
        id: usize,
        before_order: Arc<AtomicUsize>,
        after_order: Arc<AtomicUsize>,
        before_called_at: Arc<parking_lot::Mutex<Vec<usize>>>,
        after_called_at: Arc<parking_lot::Mutex<Vec<usize>>>,
    }

    impl Observer for OrderTracker {
        fn before_operation(&self, _ctx: &mut OperationContext) -> ObserverResult<()> {
            let order = self.before_order.fetch_add(1, Ordering::SeqCst);
            self.before_called_at.lock().push(order);
            Ok(())
        }

        fn after_operation(&self, _ctx: &OperationContext, _duration: Duration, _success: bool) {
            let order = self.after_order.fetch_add(1, Ordering::SeqCst);
            self.after_called_at.lock().push(order);
        }

        fn name(&self) -> &'static str {
            match self.id {
                0 => "Tracker0",
                1 => "Tracker1",
                2 => "Tracker2",
                _ => "TrackerN",
            }
        }
    }

    #[test]
    fn test_execution_order() {
        let before_order = Arc::new(AtomicUsize::new(0));
        let after_order = Arc::new(AtomicUsize::new(0));
        let before_0 = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let before_1 = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let after_0 = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let after_1 = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let mut stack = ObserverStack::new();
        stack.push(OrderTracker {
            id: 0,
            before_order: Arc::clone(&before_order),
            after_order: Arc::clone(&after_order),
            before_called_at: Arc::clone(&before_0),
            after_called_at: Arc::clone(&after_0),
        });
        stack.push(OrderTracker {
            id: 1,
            before_order: Arc::clone(&before_order),
            after_order: Arc::clone(&after_order),
            before_called_at: Arc::clone(&before_1),
            after_called_at: Arc::clone(&after_1),
        });

        let mut ctx = OperationContext::new("SELECT 1");
        stack.before_operation(&mut ctx).unwrap();
        stack.after_operation(&ctx, Duration::from_millis(10), true);

        assert_eq!(*before_0.lock(), vec![0]);
        assert_eq!(*before_1.lock(), vec![1]);
        assert_eq!(*after_1.lock(), vec![0]);
        assert_eq!(*after_0.lock(), vec![1]);
    }

    #[test]
    fn test_empty_stack() {
        let stack = ObserverStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);

        let mut ctx = OperationContext::new("SELECT 1");
        assert!(stack.before_operation(&mut ctx).is_ok());
        stack.after_operation(&ctx, Duration::from_millis(10), true);
    }

    #[test]
    fn test_names() {
        let mut stack = ObserverStack::new();
        stack.push(OrderTracker {
            id: 0,
            before_order: Arc::new(AtomicUsize::new(0)),
            after_order: Arc::new(AtomicUsize::new(0)),
            before_called_at: Arc::new(parking_lot::Mutex::new(Vec::new())),
            after_called_at: Arc::new(parking_lot::Mutex::new(Vec::new())),
        });
        stack.push(OrderTracker {
            id: 1,
            before_order: Arc::new(AtomicUsize::new(0)),
            after_order: Arc::new(AtomicUsize::new(0)),
            before_called_at: Arc::new(parking_lot::Mutex::new(Vec::new())),
            after_called_at: Arc::new(parking_lot::Mutex::new(Vec::new())),
        });

        assert_eq!(stack.names(), vec!["Tracker0", "Tracker1"]);
    }

    #[test]
    fn test_clear() {
        let mut stack = ObserverStack::new();
        stack.push(OrderTracker {
            id: 0,
            before_order: Arc::new(AtomicUsize::new(0)),
            after_order: Arc::new(AtomicUsize::new(0)),
            before_called_at: Arc::new(parking_lot::Mutex::new(Vec::new())),
            after_called_at: Arc::new(parking_lot::Mutex::new(Vec::new())),
        });
        assert_eq!(stack.len(), 1);
        stack.clear();
        assert!(stack.is_empty());
    }
}
