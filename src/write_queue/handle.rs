//! Handle for awaiting deferred write results.

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::sync::oneshot;

use crate::error::PoolError;

/// Error type for write queue operations.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    /// Queue is at capacity and overflow policy rejected the write.
    #[error("write queue is full (capacity: {capacity}, policy rejected)")]
    QueueFull { capacity: usize },

    /// Write was dropped due to `DropOldest` policy.
    #[error("write was dropped due to queue overflow (DropOldest policy)")]
    Dropped,

    /// Queue has been shut down.
    #[error("write queue has been shut down")]
    Shutdown,

    /// Internal channel error.
    #[error("internal queue error: {0}")]
    Internal(String),

    /// The write operation itself failed.
    #[error(transparent)]
    Write(#[from] PoolError),
}

/// Type-erased result from a write operation.
pub(crate) type ErasedResult = Result<Box<dyn Any + Send>, PoolError>;

/// Handle for awaiting the result of a queued write operation.
///
/// The handle can be awaited to get the result once the pool-backed write queue
/// processes it. If the write was dropped due to `DropOldest` or immediate
/// shutdown, awaiting returns `QueueError::Dropped`.
///
/// # Example
///
/// ```rust,ignore
/// let handle: WriteHandle<i64> = queue.enqueue(|conn| {
///     conn.query_row(
///         "INSERT INTO users (name) VALUES (?) RETURNING id",
///         ["Alice"],
///         |row| row.get(0)
///     )
/// }).await?;
///
/// let user_id: i64 = handle.await?;
/// ```
pub struct WriteHandle<T> {
    receiver: oneshot::Receiver<ErasedResult>,
    _marker: PhantomData<T>,
}

impl<T> WriteHandle<T> {
    /// Create a new write handle from a oneshot receiver.
    pub(crate) fn new(receiver: oneshot::Receiver<ErasedResult>) -> Self {
        Self {
            receiver,
            _marker: PhantomData,
        }
    }

    /// Check if the result is ready without blocking.
    ///
    /// Returns `None` if the write is still pending.
    pub fn try_get(&mut self) -> Option<Result<T, QueueError>>
    where
        T: Send + 'static,
    {
        match self.receiver.try_recv() {
            Ok(result) => Some(convert_result(result)),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(QueueError::Dropped)),
        }
    }

    /// Check if the write has completed (success or failure).
    ///
    /// This only reports readiness; it does not distinguish success from error.
    pub fn is_ready(&self) -> bool {
        self.receiver.is_terminated()
    }
}

impl<T> Unpin for WriteHandle<T> {}

impl<T: Send + 'static> Future for WriteHandle<T> {
    type Output = Result<T, QueueError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.receiver).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(convert_result(result)),
            Poll::Ready(Err(_)) => Poll::Ready(Err(QueueError::Dropped)),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn convert_result<T: Send + 'static>(result: ErasedResult) -> Result<T, QueueError> {
    match result {
        Ok(any) => {
            let typed = any
                .downcast::<T>()
                .map_err(|_| QueueError::Internal("WriteHandle type mismatch".to_string()))?;
            Ok(*typed)
        }
        Err(e) => Err(QueueError::Write(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_write_handle_receives_result() {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(Box::new(42i64) as Box<dyn Any + Send>));

        let handle: WriteHandle<i64> = WriteHandle::new(rx);
        let result = handle.await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_write_handle_dropped_error() {
        let (tx, rx) = oneshot::channel::<ErasedResult>();
        drop(tx);

        let handle: WriteHandle<i64> = WriteHandle::new(rx);
        let result = handle.await;

        assert!(matches!(result, Err(QueueError::Dropped)));
    }

    #[test]
    fn test_try_get_pending() {
        let (_tx, rx) = oneshot::channel::<ErasedResult>();
        let mut handle: WriteHandle<i64> = WriteHandle::new(rx);

        assert!(handle.try_get().is_none());
    }

    #[test]
    fn test_try_get_ready() {
        let (tx, rx) = oneshot::channel::<ErasedResult>();
        let _ = tx.send(Ok(Box::new(42i64) as Box<dyn Any + Send>));

        let mut handle: WriteHandle<i64> = WriteHandle::new(rx);
        let result = handle.try_get();

        assert!(matches!(result, Some(Ok(42))));
    }
}
