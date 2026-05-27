//! Write queue integration with DatabasePool.
//!
//! This module provides [`PoolWriteQueue`], a write queue that integrates with
//! an existing [`DatabasePool`] instead of owning its own connection.

use std::any::Any;
use std::sync::Arc;

use tokio::sync::oneshot;

use super::bounded::{BoundedQueue, QueueTask, WakeReason, Worker};
use super::config::WriteQueueConfig;
use super::handle::{QueueError, WriteHandle};
use super::stats::QueueStats;
use crate::pool::DatabasePool;
use crate::PoolError;

/// Type-erased write operation for the pool queue.
type PoolWriteOp = Box<
    dyn FnOnce(&mut rusqlite::Connection) -> Result<Box<dyn Any + Send>, rusqlite::Error> + Send,
>;

/// Internal task for the pool queue.
///
/// Internal task dispatched through `DatabasePool::write`.
struct PoolWriteTask {
    operation: PoolWriteOp,
    response: Option<oneshot::Sender<Result<Box<dyn Any + Send>, PoolError>>>,
}

impl QueueTask for PoolWriteTask {
    fn notify_dropped(self) {
        // Dropping the sender causes the receiver to observe `Closed`, which
        // `WriteHandle` maps to `QueueError::Dropped`.
        drop(self.response);
    }
}

/// A write queue that works with an existing [`DatabasePool`].
///
/// `PoolWriteQueue` uses an existing pool's writer connection instead of
/// owning a second writer. This keeps write ownership centralized in
/// [`DatabasePool`].
///
/// # Example
///
/// ```rust,ignore
/// use sqlite_rs::{DatabasePool, PoolConfig};
/// use sqlite_rs::write_queue::{PoolWriteQueue, WriteQueueConfig};
/// use std::sync::Arc;
///
/// let pool = Arc::new(DatabasePool::open("data.db", PoolConfig::default()).await?);
///
/// // Create a queue backed by the pool
/// let queue = PoolWriteQueue::new(Arc::clone(&pool), WriteQueueConfig::default());
///
/// // Fire-and-forget writes go through the queue
/// queue.fire_and_forget(|conn| {
///     conn.execute("INSERT INTO logs (msg) VALUES (?)", ["event"])?;
///     Ok(())
/// }).await?;
///
/// // Direct pool.write() still works for immediate writes
/// pool.write(|conn| {
///     conn.execute("INSERT INTO critical (value) VALUES (?)", [42])
/// }).await?;
///
/// // Graceful shutdown
/// queue.shutdown().await;
/// ```
///
/// # Thread Safety
///
/// The queue is `Send + Sync` and can be shared across tasks. Internally it
/// serializes writes through the pool's writer connection.
pub struct PoolWriteQueue {
    pool: Arc<DatabasePool>,
    queue: BoundedQueue<PoolWriteTask>,
    worker_handle: Option<tokio::task::JoinHandle<()>>,
}

impl PoolWriteQueue {
    /// Create a new write queue backed by the given pool.
    ///
    /// The queue will use the pool's writer connection to process writes.
    /// The pool must be wrapped in an `Arc` to share ownership.
    pub fn new(pool: Arc<DatabasePool>, config: WriteQueueConfig) -> Self {
        let (queue, shutdown_rx) = BoundedQueue::new(config);
        let worker = queue.worker();
        let worker_pool = Arc::clone(&pool);
        let worker_handle = tokio::spawn(async move {
            pool_worker_loop(worker, worker_pool, shutdown_rx).await;
        });

        Self {
            pool,
            queue,
            worker_handle: Some(worker_handle),
        }
    }

    /// Enqueue a write operation and get a handle to await the result.
    pub async fn enqueue<F, T>(&self, f: F) -> Result<WriteHandle<T>, QueueError>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, rusqlite::Error> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let operation: PoolWriteOp = Box::new(move |conn| {
            let result = f(conn)?;
            Ok(Box::new(result) as Box<dyn Any + Send>)
        });
        let task = PoolWriteTask {
            operation,
            response: Some(tx),
        };
        self.queue.enqueue(task).await?;
        Ok(WriteHandle::new(rx))
    }

    /// Queue a write operation without waiting for the result.
    pub async fn fire_and_forget<F>(&self, f: F) -> Result<(), QueueError>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<(), rusqlite::Error> + Send + 'static,
    {
        let operation: PoolWriteOp = Box::new(move |conn| {
            f(conn)?;
            Ok(Box::new(()) as Box<dyn Any + Send>)
        });
        let task = PoolWriteTask {
            operation,
            response: None,
        };
        self.queue.enqueue(task).await
    }

    /// Enqueue a transaction and get a handle to await the result.
    ///
    /// The transaction is automatically committed if the function returns Ok,
    /// or rolled back if it returns Err.
    pub async fn enqueue_transaction<F, T>(&self, f: F) -> Result<WriteHandle<T>, QueueError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, rusqlite::Error> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let operation: PoolWriteOp = Box::new(move |conn| {
            let db_tx = conn.transaction()?;
            let result = f(&db_tx)?;
            db_tx.commit()?;
            Ok(Box::new(result) as Box<dyn Any + Send>)
        });
        let task = PoolWriteTask {
            operation,
            response: Some(tx),
        };
        self.queue.enqueue(task).await?;
        Ok(WriteHandle::new(rx))
    }

    /// Get current queue statistics.
    pub fn stats(&self) -> QueueStats {
        self.queue.stats()
    }

    /// Get number of pending writes.
    pub fn pending(&self) -> u64 {
        self.queue.pending()
    }

    /// Check if queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Check if queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.queue.is_full()
    }

    /// Get a reference to the underlying pool.
    pub fn pool(&self) -> &DatabasePool {
        &self.pool
    }

    /// Gracefully shut down the queue, processing all pending writes.
    pub async fn shutdown(mut self) {
        self.queue.signal_shutdown();
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.await;
        }
    }

    /// Shut down immediately, dropping any pending writes.
    pub async fn shutdown_immediate(mut self) {
        self.queue.evict_all();
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.await;
        }
    }
}

/// Background worker for the pool queue.
async fn pool_worker_loop(
    worker: Worker<PoolWriteTask>,
    pool: Arc<DatabasePool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        // Drain before waiting. `Notify` is not a counting channel, so this
        // prevents a task enqueued before the worker parks from waiting for a
        // later notification.
        process_pool_tasks(&worker, &pool).await;

        if worker.is_shutdown() {
            drain_pool_queue(&worker, &pool).await;
            break;
        }

        match worker.wait(&mut shutdown_rx).await {
            WakeReason::Shutdown => {
                drain_pool_queue(&worker, &pool).await;
                break;
            }
            WakeReason::Tasks => {}
        }
    }

    crate::telemetry::info!("pool write queue worker shut down");
}

/// Process available tasks.
async fn process_pool_tasks(worker: &Worker<PoolWriteTask>, pool: &DatabasePool) {
    use std::sync::atomic::{AtomicBool, Ordering};

    while let Some(task) = worker.pop() {
        let has_response = task.response.is_some();
        let operation = task.operation;
        let response = task.response;

        let op_succeeded = Arc::new(AtomicBool::new(false));
        let op_succeeded_clone = Arc::clone(&op_succeeded);

        let result = pool
            .write(move |conn| {
                let result = operation(conn);
                op_succeeded_clone.store(result.is_ok(), Ordering::SeqCst);
                if let Some(tx) = response {
                    let send_result = result.map_err(PoolError::from);
                    let _ = tx.send(send_result);
                    Ok(())
                } else {
                    result.map(|_| ())
                }
            })
            .await;

        let operation_failed = !op_succeeded.load(Ordering::SeqCst);
        worker.complete(result.is_err() || operation_failed);

        if !has_response && result.is_err() {
            if let Err(ref _e) = result {
                crate::telemetry::warn!(
                    error = %crate::telemetry::chain(_e),
                    "pool queue fire-and-forget write failed"
                );
            }
        }
    }
}

/// Drain remaining tasks for shutdown.
async fn drain_pool_queue(worker: &Worker<PoolWriteTask>, pool: &DatabasePool) {
    while worker.pending() > 0 {
        process_pool_tasks(worker, pool).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PoolConfig;
    use std::time::Duration;
    use tempfile::tempdir;

    async fn setup_pool_queue(config: WriteQueueConfig) -> (PoolWriteQueue, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = Arc::new(
            DatabasePool::open(&db_path, PoolConfig::minimal())
                .await
                .unwrap(),
        );

        pool.write(|conn| {
            conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
        })
        .await
        .unwrap();

        let queue = PoolWriteQueue::new(pool, config);
        (queue, dir)
    }

    #[tokio::test]
    #[should_panic(expected = "write queue capacity must be greater than 0")]
    async fn test_pool_queue_zero_capacity_config_rejected() {
        let config = WriteQueueConfig {
            capacity: 0,
            overflow_policy: super::super::config::OverflowPolicy::Reject,
        };

        let (_queue, _dir) = setup_pool_queue(config).await;
    }

    #[tokio::test]
    async fn test_pool_queue_basic() {
        let (queue, _dir) = setup_pool_queue(WriteQueueConfig::default()).await;

        let handle = queue
            .enqueue(|conn| conn.execute("INSERT INTO test (value) VALUES (?)", ["hello"]))
            .await
            .unwrap();

        let rows = handle.await.unwrap();
        assert_eq!(rows, 1);

        queue.shutdown().await;
    }

    #[tokio::test]
    async fn test_pool_queue_fire_and_forget() {
        let (queue, _dir) = setup_pool_queue(WriteQueueConfig::default()).await;

        queue
            .fire_and_forget(|conn| {
                conn.execute("INSERT INTO test (value) VALUES (?)", ["event"])?;
                Ok(())
            })
            .await
            .unwrap();

        let mut stats = queue.stats();
        for _ in 0..20 {
            if stats.processed == 1 && stats.pending == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            stats = queue.stats();
        }

        assert_eq!(stats.processed, 1);
        assert_eq!(stats.pending, 0);

        queue.shutdown().await;
    }

    #[tokio::test]
    async fn test_pool_queue_counts_awaitable_write_errors() {
        let (queue, _dir) = setup_pool_queue(WriteQueueConfig::default()).await;

        let handle = queue
            .enqueue(|conn| conn.execute("INSERT INTO missing_table (value) VALUES ('bad')", []))
            .await
            .unwrap();

        let result = handle.await;
        assert!(matches!(
            result,
            Err(QueueError::Write(PoolError::Sqlite(_)))
        ));

        let mut stats = queue.stats();
        for _ in 0..10 {
            if stats.processed == 1 && stats.errors == 1 && stats.pending == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            stats = queue.stats();
        }

        assert_eq!(stats.processed, 1);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.pending, 0);

        queue.shutdown().await;
    }

    #[tokio::test]
    async fn test_pool_queue_drop_without_shutdown_wakes_worker_and_drains_queue() {
        let (queue, _dir) = setup_pool_queue(WriteQueueConfig::default()).await;

        let handle = queue
            .enqueue(|conn| conn.execute("INSERT INTO test (value) VALUES (?)", ["drop-drain"]))
            .await
            .unwrap();

        drop(queue);

        let rows = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker should exit instead of waiting forever after sender drop")
            .unwrap();

        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn test_pool_queue_with_direct_writes() {
        let (queue, _dir) = setup_pool_queue(WriteQueueConfig::default()).await;

        let handle = queue
            .enqueue(|conn| conn.execute("INSERT INTO test (value) VALUES (?)", ["queued"]))
            .await
            .unwrap();

        queue
            .pool()
            .write(|conn| conn.execute("INSERT INTO test (value) VALUES (?)", ["direct"]))
            .await
            .unwrap();

        let _ = handle.await.unwrap();

        let count: i64 = queue
            .pool()
            .read(|conn| conn.query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0)))
            .await
            .unwrap();

        assert_eq!(count, 2);

        queue.shutdown().await;
    }
}
