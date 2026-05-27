//! Optional pool-backed write queue for coordinating concurrent write pressure.
//!
//! SQLite only allows one writer at a time. [`DatabasePool`](crate::DatabasePool)
//! already serializes writes through its dedicated writer connection; this module
//! adds an optional bounded queue on top when callers need explicit backpressure,
//! deferred write handles, or drop/reject overflow behavior.
//!
//! # When to Use
//!
//! Use [`PoolWriteQueue`] when many tasks produce writes faster than the writer
//! can process them and you want a visible queue boundary. For latency-sensitive
//! or critical writes, direct `pool.write(...)` remains the simpler path.
//!
//! # Ordering Guarantees
//!
//! - **FIFO**: Writes are processed in the order they are enqueued.
//! - **No interleaving**: Each queued write completes before the next begins.
//! - **Transaction atomicity**: `enqueue_transaction()` executes atomically.
//!
//! # Shutdown Behavior
//!
//! - [`PoolWriteQueue::shutdown()`]: stops accepting new writes and drains pending writes.
//! - [`PoolWriteQueue::shutdown_immediate()`]: drops pending writes; handles receive `Dropped`.
//!
//! # Handle Cancellation
//!
//! If a [`WriteHandle`] is dropped before awaiting, the write still executes and
//! its result is discarded. There is no cancellation for a queued write after it
//! has been accepted.
//!
//! # Error Handling
//!
//! - Queue errors (`full`, `shutdown`) are returned from `enqueue()` / `fire_and_forget()`.
//! - Write errors are returned when awaiting the [`WriteHandle`].
//! - Fire-and-forget errors are logged via the crate telemetry facade.
//!
//! # Architecture
//!
//! ```text
//! DatabasePool writer
//!        ▲
//!        │ pool.write(...)
//!        │
//! PoolWriteQueue worker
//!        ▲
//!        │
//! BoundedQueue<Task>  ← producers
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use sqlite_rs::write_queue::{OverflowPolicy, PoolWriteQueue, WriteQueueConfig};
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! let config = WriteQueueConfig {
//!     capacity: 500,
//!     overflow_policy: OverflowPolicy::BlockTimeout(Duration::from_secs(5)),
//! };
//!
//! let queue = PoolWriteQueue::new(Arc::clone(&pool), config);
//!
//! queue.fire_and_forget(|conn| {
//!     conn.execute("INSERT INTO logs (msg) VALUES (?)", ["event"])?;
//!     Ok(())
//! }).await?;
//!
//! let handle = queue.enqueue(|conn| {
//!     conn.execute("INSERT INTO critical_data (value) VALUES (?)", [42])
//! }).await?;
//! let rows_affected = handle.await?;
//! ```

mod bounded;
mod config;
mod handle;
mod pool_queue;
mod stats;

pub use config::{OverflowPolicy, WriteQueueConfig};
pub use handle::{QueueError, WriteHandle};
pub use pool_queue::PoolWriteQueue;
pub use stats::QueueStats;
