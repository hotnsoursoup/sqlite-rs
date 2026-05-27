//! Bounded MPSC queue with pluggable overflow policy.
//!
//! Shared primitive used by [`PoolWriteQueue`](super::PoolWriteQueue). Enqueue,
//! overflow handling, lifecycle, and statistics live here so execution remains
//! focused in the pool queue worker.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{watch, Notify, Semaphore, TryAcquireError};

use super::config::{OverflowPolicy, WriteQueueConfig};
use super::handle::QueueError;
use super::stats::QueueStats;

/// Items stored in a [`BoundedQueue`].
///
/// The queue uses `notify_dropped` to release any waiter the task is holding
/// when overflow policy evicts the task before the worker can run it.
pub(crate) trait QueueTask: Send + 'static {
    /// Called when this task is removed from the queue without being executed
    /// (overflow eviction or immediate shutdown). Implementations should signal
    /// any waiter that the task will never produce a result.
    fn notify_dropped(self);
}

/// Shared state between the queue and its worker.
struct State<T> {
    /// Pending tasks. `VecDeque` so `DropOldest` can pop from the front.
    tasks: Mutex<VecDeque<T>>,
    /// Tracks remaining capacity. Permits are forgotten on enqueue and
    /// re-added on completion / eviction.
    capacity: Semaphore,
    /// Wakes the worker when a task arrives.
    notify: Notify,
    pending: AtomicU64,
    processed: AtomicU64,
    dropped: AtomicU64,
    errors: AtomicU64,
    config: WriteQueueConfig,
    shutdown: AtomicBool,
}

/// A bounded queue with configurable overflow behaviour and built-in statistics.
///
/// `BoundedQueue` is the storage and lifecycle primitive. It does not know how
/// to execute its items — a worker task receives a [`Worker`] handle and drains
/// the queue against whatever execution surface is appropriate.
pub(crate) struct BoundedQueue<T: QueueTask> {
    state: Arc<State<T>>,
    shutdown_tx: watch::Sender<bool>,
}

impl<T: QueueTask> BoundedQueue<T> {
    /// Create a new queue with the given configuration. Returns the queue and
    /// the receiver half of the shutdown signal that the worker should select
    /// on.
    pub(crate) fn new(config: WriteQueueConfig) -> (Self, watch::Receiver<bool>) {
        assert!(
            config.capacity > 0,
            "write queue capacity must be greater than 0"
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = Arc::new(State {
            tasks: Mutex::new(VecDeque::with_capacity(config.capacity)),
            capacity: Semaphore::new(config.capacity),
            notify: Notify::new(),
            pending: AtomicU64::new(0),
            processed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            config,
            shutdown: AtomicBool::new(false),
        });
        (Self { state, shutdown_tx }, shutdown_rx)
    }

    /// A worker handle that lets a spawned task drain the queue.
    pub(crate) fn worker(&self) -> Worker<T> {
        Worker {
            state: Arc::clone(&self.state),
        }
    }

    /// Submit a task to the queue, honouring the configured overflow policy.
    pub(crate) async fn enqueue(&self, task: T) -> Result<(), QueueError> {
        if self.state.shutdown.load(Ordering::SeqCst) {
            return Err(QueueError::Shutdown);
        }
        acquire_slot(&self.state, &self.state.config.overflow_policy).await?;
        if self.state.shutdown.load(Ordering::SeqCst) {
            self.state.capacity.add_permits(1);
            return Err(QueueError::Shutdown);
        }
        self.state.tasks.lock().push_back(task);
        self.state.pending.fetch_add(1, Ordering::SeqCst);
        self.state.notify.notify_one();
        Ok(())
    }

    /// Signal the worker to stop. Pending tasks are left in the queue and will
    /// be drained by the worker before it exits.
    pub(crate) fn signal_shutdown(&self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        self.state.capacity.close();
        let _ = self.shutdown_tx.send(true);
        // Wake the worker so it observes the signal even when idle.
        self.state.notify.notify_one();
    }

    /// Signal shutdown and synchronously evict every pending task, notifying
    /// each one's waiter. Used by `shutdown_immediate`.
    pub(crate) fn evict_all(&self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        self.state.capacity.close();
        let _ = self.shutdown_tx.send(true);
        let drained: Vec<T> = self.state.tasks.lock().drain(..).collect();
        let drained_count = drained.len() as u64;
        for task in drained {
            task.notify_dropped();
            self.state.dropped.fetch_add(1, Ordering::Relaxed);
        }
        if drained_count > 0 {
            self.state
                .pending
                .fetch_sub(drained_count, Ordering::SeqCst);
        }
        self.state.notify.notify_one();
    }

    pub(crate) fn stats(&self) -> QueueStats {
        QueueStats {
            pending: self.state.pending.load(Ordering::Relaxed),
            processed: self.state.processed.load(Ordering::Relaxed),
            dropped: self.state.dropped.load(Ordering::Relaxed),
            errors: self.state.errors.load(Ordering::Relaxed),
            capacity: self.state.config.capacity,
        }
    }

    pub(crate) fn pending(&self) -> u64 {
        self.state.pending.load(Ordering::Relaxed)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pending() == 0
    }

    pub(crate) fn is_full(&self) -> bool {
        self.pending() as usize >= self.state.config.capacity
    }
}

/// A clonable handle to the queue used by the worker task.
///
/// The worker uses this to dequeue tasks, wait for new arrivals, react to
/// shutdown, and report progress.
pub(crate) struct Worker<T: QueueTask> {
    state: Arc<State<T>>,
}

impl<T: QueueTask> Worker<T> {
    /// Pop the next task from the queue, or `None` if empty.
    pub(crate) fn pop(&self) -> Option<T> {
        self.state.tasks.lock().pop_front()
    }

    /// After a task is processed: decrement pending, increment processed,
    /// optionally increment the error counter, and return one capacity permit.
    pub(crate) fn complete(&self, errored: bool) {
        self.state.pending.fetch_sub(1, Ordering::SeqCst);
        self.state.processed.fetch_add(1, Ordering::SeqCst);
        if errored {
            self.state.errors.fetch_add(1, Ordering::SeqCst);
        }
        self.state.capacity.add_permits(1);
    }

    /// Wait until either a task arrives or shutdown is signalled.
    ///
    /// Returns the [`WakeReason`] so the caller can decide whether to drain
    /// the queue one more time or exit immediately.
    pub(crate) async fn wait(&self, shutdown_rx: &mut watch::Receiver<bool>) -> WakeReason {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => match changed {
                Ok(()) if *shutdown_rx.borrow() => WakeReason::Shutdown,
                Ok(()) => WakeReason::Tasks,
                Err(_) => WakeReason::Shutdown,
            },
            _ = self.state.notify.notified() => WakeReason::Tasks,
        }
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.state.shutdown.load(Ordering::SeqCst)
    }

    pub(crate) fn pending(&self) -> u64 {
        self.state.pending.load(Ordering::Relaxed)
    }
}

/// Reason a [`Worker::wait`] call returned.
pub(crate) enum WakeReason {
    /// A task is (likely) available, or the worker should re-check the queue.
    Tasks,
    /// Shutdown was signalled; drain remaining tasks and exit.
    Shutdown,
}

/// Acquire one slot from the capacity semaphore using the configured policy.
///
/// On success the permit is `forget()`-ed; capacity is returned to the
/// semaphore later, either when the task is processed
/// ([`Worker::complete`]) or when it is evicted (`DropOldest`).
async fn acquire_slot<T: QueueTask>(
    state: &Arc<State<T>>,
    policy: &OverflowPolicy,
) -> Result<(), QueueError> {
    match policy {
        OverflowPolicy::Block => {
            let permit = state
                .capacity
                .acquire()
                .await
                .map_err(|_| QueueError::Shutdown)?;
            permit.forget();
            Ok(())
        }
        OverflowPolicy::BlockTimeout(duration) => {
            match tokio::time::timeout(*duration, state.capacity.acquire()).await {
                Ok(Ok(permit)) => {
                    permit.forget();
                    Ok(())
                }
                Ok(Err(_)) => Err(QueueError::Shutdown),
                Err(_) => Err(QueueError::QueueFull {
                    capacity: state.config.capacity,
                }),
            }
        }
        OverflowPolicy::Reject => match state.capacity.try_acquire() {
            Ok(permit) => {
                permit.forget();
                Ok(())
            }
            Err(TryAcquireError::Closed) => Err(QueueError::Shutdown),
            Err(TryAcquireError::NoPermits) => Err(QueueError::QueueFull {
                capacity: state.config.capacity,
            }),
        },
        OverflowPolicy::DropOldest => {
            // Loop with a bounded retry budget: each iteration either acquires
            // a permit or evicts the oldest queued task to free one. In-flight
            // tasks are not droppable, so an empty queue with all capacity held
            // by workers must reject instead of minting a phantom permit.
            const MAX_DROP_ATTEMPTS: usize = 10;
            for _ in 0..=MAX_DROP_ATTEMPTS {
                if state.shutdown.load(Ordering::SeqCst) {
                    return Err(QueueError::Shutdown);
                }

                match state.capacity.try_acquire() {
                    Ok(permit) => {
                        permit.forget();
                        return Ok(());
                    }
                    Err(TryAcquireError::Closed) => return Err(QueueError::Shutdown),
                    Err(TryAcquireError::NoPermits) => {}
                }

                if state.shutdown.load(Ordering::SeqCst) {
                    return Err(QueueError::Shutdown);
                }

                let dropped = state.tasks.lock().pop_front();
                if let Some(task) = dropped {
                    task.notify_dropped();
                    state.dropped.fetch_add(1, Ordering::SeqCst);
                    state.pending.fetch_sub(1, Ordering::SeqCst);
                    state.capacity.add_permits(1);
                } else {
                    // The worker may have popped the only task and not yet
                    // returned its permit. Yield once and retry; if capacity is
                    // still held by in-flight work, the queue is full.
                    tokio::task::yield_now().await;
                }
            }
            Err(QueueError::QueueFull {
                capacity: state.config.capacity,
            })
        }
    }
}
