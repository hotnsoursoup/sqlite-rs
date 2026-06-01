//! The main database pool with read/write split architecture.

use crate::config::PoolConfig;
use crate::connection::{apply_init_hook, configure_connection, AsyncConnection};
use crate::error::{PoolError, Result};
use crate::migrations::{run_migrations_with_options, Migration, MigrationOptions};
use crate::observer::{ObserverStack, OperationContext, OperationKind};

use deadpool::managed::{Hook, HookError};
use deadpool_sqlite::{Config as DeadpoolConfig, Object as PooledConn, Pool, Runtime};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Semaphore, SemaphorePermit};

/// Global counter for unique in-memory database names.
static IN_MEMORY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "wal-monitor")]
use crate::wal::spawn_wal_monitor;

/// High-performance SQLite connection pool with read/write split.
///
/// # Architecture
///
/// - **Writer**: A single dedicated connection for all write operations.
///   Writes are serialized to prevent lock contention.
/// - **Readers**: A pool of connections for concurrent read operations.
///   Multiple readers can execute simultaneously.
/// - **WAL Monitor**: Optional background task that monitors WAL file size.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::{DatabasePool, PoolConfig};
///
/// # async fn example() -> Result<(), sqlite_kit::PoolError> {
/// let pool = DatabasePool::open("data/app.db", PoolConfig::default()).await?;
///
/// // Concurrent reads
/// let count = pool.read(|conn| {
///     conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get::<_, i64>(0))
/// }).await?;
///
/// // Serialized writes
/// pool.write(|conn| {
///     conn.execute("INSERT INTO users (name) VALUES (?)", ["Alice"])
/// }).await?;
///
/// pool.close().await?;
/// # Ok(())
/// # }
/// ```
pub struct DatabasePool {
    /// Dedicated writer connection (serialized access).
    /// Stored as an option so `close()` can consume it and surface close errors
    /// while `Drop` can still fall back to best-effort channel shutdown.
    writer: Option<AsyncConnection>,

    /// Reader connection pool (concurrent access).
    readers: Pool,

    /// Path to the database file.
    db_path: PathBuf,

    /// Pool configuration.
    config: PoolConfig,

    /// Shutdown signal for WAL monitor.
    wal_shutdown: Option<watch::Sender<bool>>,

    /// Whether the pool is closed.
    closed: AtomicBool,

    /// Semaphore tracking in-flight operations.
    /// Acquire a permit for each operation, release on completion.
    /// This ensures graceful shutdown by preventing new operations after close
    /// and waiting for existing operations to complete.
    in_flight_ops: Arc<Semaphore>,

    /// Operation observers for profiling, logging, metrics, and policy hooks.
    observers: ObserverStack,
}

impl DatabasePool {
    /// Open a database pool with the given configuration.
    pub async fn open<P: AsRef<Path>>(path: P, config: PoolConfig) -> Result<Self> {
        let path = normalize_database_path(path.as_ref());

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        crate::telemetry::info!(
            path = %path.display(),
            readers = config.reader_count,
            "opening database pool"
        );

        // Open dedicated writer connection
        let writer = AsyncConnection::open(&path, &config)
            .await
            .map_err(|e| PoolError::WriterOpen(e.to_string()))?;

        // Create reader pool using deadpool. Reader init runs once when a
        // connection is created; per-read code only reapplies cheap pragmas.
        let deadpool_config = DeadpoolConfig::new(&path);
        let reader_config = config.clone();
        let readers = deadpool_config
            .builder(Runtime::Tokio1)
            .map_err(|e| PoolError::PoolCreate(e.to_string()))?
            .max_size(config.reader_count)
            .post_create(Hook::<deadpool_sqlite::Manager>::async_fn(
                move |conn: &mut <deadpool_sqlite::Manager as deadpool::managed::Manager>::Type,
                      _| {
                    let config = reader_config.clone();
                    Box::pin(async move {
                        let result = conn
                            .interact(move |conn: &mut rusqlite::Connection| {
                                conn.execute_batch("PRAGMA journal_mode = WAL;")?;
                                configure_connection(conn, &config)?;
                                apply_init_hook(conn, &config.init_hook)
                            })
                            .await
                            .map_err(|e| {
                                HookError::message(format!("reader init failed: {}", e))
                            })?;

                        result.map_err(HookError::Backend)
                    })
                },
            ))
            .build()
            .map_err(|e| PoolError::PoolCreate(e.to_string()))?;

        // Start WAL monitor if configured and compiled in.
        #[cfg(feature = "wal-monitor")]
        let wal_shutdown = if let Some(ref wal_config) = config.wal {
            let (tx, rx) = watch::channel(false);
            spawn_wal_monitor(path.clone(), rx, wal_config.clone());
            Some(tx)
        } else {
            None
        };

        #[cfg(not(feature = "wal-monitor"))]
        let wal_shutdown = None;

        // Clone observers before storing config; the stack contains shared Arcs.
        let observers = config.observers.clone();

        Ok(Self {
            writer: Some(writer),
            readers,
            db_path: path,
            config,
            wal_shutdown,
            closed: AtomicBool::new(false),
            in_flight_ops: Arc::new(Semaphore::new(1000)),
            observers,
        })
    }

    /// Open a database pool with default configuration.
    pub async fn open_default<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open(path, PoolConfig::default()).await
    }

    /// Open an in-memory database pool.
    ///
    /// Useful for testing. Each call creates a unique shared-cache in-memory
    /// database so the writer and reader connections see the same schema/data.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> Result<(), sqlite_kit::PoolError> {
    /// let pool = sqlite_kit::DatabasePool::open_in_memory().await?;
    /// // Use pool for testing...
    /// # Ok(())
    /// # }
    /// ```
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(unique_in_memory_uri(), PoolConfig::minimal()).await
    }

    /// Run migrations on the database.
    pub async fn migrate(&self, migrations: &[Migration]) -> Result<()> {
        self.migrate_with_options(migrations, MigrationOptions::new())
            .await
    }

    /// Run migrations on the database with custom options.
    pub async fn migrate_with_options(
        &self,
        migrations: &[Migration],
        options: MigrationOptions,
    ) -> Result<()> {
        let guard = self
            .begin_op("pool.migrate", OperationKind::Ddl, false)
            .await?;
        let migrations: Vec<Migration> = migrations.to_vec();

        let result = self
            .writer()?
            .call(move |conn| {
                run_migrations_with_options(conn, &migrations, options)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })
            .await
            .map(|_| ())
            .map_err(|e| PoolError::Migration(e.to_string()));

        guard.complete_result(&result);
        result
    }

    /// Execute a read operation.
    ///
    /// Read operations run concurrently on the reader pool.
    /// Configured observers are notified before and after the operation.
    pub async fn read<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&rusqlite::Connection) -> std::result::Result<R, rusqlite::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let guard = self
            .begin_op("pool.read", OperationKind::Select, true)
            .await?;
        let conn = match self.acquire_reader().await {
            Ok(conn) => conn,
            Err(err) => {
                guard.complete_error(&err);
                return Err(err);
            }
        };
        let config = self.config.clone();

        let result = conn
            .interact(move |conn| {
                // Pragmas are cheap and idempotent. Init hooks are not run
                // here because they are connection-lifetime setup.
                configure_connection(conn, &config)?;
                // Enable read-only mode for readers
                conn.execute_batch("PRAGMA query_only = ON;")?;
                f(conn)
            })
            .await
            .map_err(|e| PoolError::Interact(e.to_string()))?
            .map_err(PoolError::from);

        guard.complete_result(&result);
        result
    }

    /// Execute a write operation.
    ///
    /// Write operations are serialized through the dedicated writer connection.
    /// Configured observers are notified before and after the operation.
    pub async fn write<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> std::result::Result<R, rusqlite::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let guard = self
            .begin_op("pool.write", OperationKind::Other, false)
            .await?;
        let result = self.writer()?.call(f).await;
        guard.complete_result(&result);
        result
    }

    /// Execute a transaction (write).
    ///
    /// The transaction is automatically committed if the function returns Ok,
    /// or rolled back if it returns Err.
    /// Configured observers are notified before and after the operation.
    pub async fn transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> std::result::Result<R, rusqlite::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let guard = self
            .begin_op("pool.transaction", OperationKind::Transaction, false)
            .await?;
        let result = self.writer()?.transaction(f).await;
        guard.complete_result(&result);
        result
    }

    /// Execute a read-only transaction.
    ///
    /// This provides a consistent view of the database for multiple read queries.
    /// The transaction runs on a reader connection and is automatically rolled back
    /// when the function returns (since it's read-only).
    ///
    /// Use this when you need snapshot isolation for reads - all queries within
    /// the transaction see the same database state.
    /// Configured observers are notified before and after the operation.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use sqlite_kit::DatabasePool;
    ///
    /// # async fn example(pool: &DatabasePool) -> Result<(), sqlite_kit::PoolError> {
    /// // Consistent read of related data
    /// let (user, orders) = pool.read_transaction(|tx| {
    ///     let user: String = tx.query_row(
    ///         "SELECT name FROM users WHERE id = ?", [1], |row| row.get(0)
    ///     )?;
    ///
    ///     let mut stmt = tx.prepare("SELECT id FROM orders WHERE user_id = ?")?;
    ///     let orders: Vec<i64> = stmt
    ///         .query_map([1], |row| row.get(0))?
    ///         .collect::<Result<_, _>>()?;
    ///
    ///     Ok((user, orders))
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn read_transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> std::result::Result<R, rusqlite::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let guard = self
            .begin_op("pool.read_transaction", OperationKind::Select, true)
            .await?;
        let conn = match self.acquire_reader().await {
            Ok(conn) => conn,
            Err(err) => {
                guard.complete_error(&err);
                return Err(err);
            }
        };
        let config = self.config.clone();

        let result = conn
            .interact(move |conn| -> std::result::Result<R, rusqlite::Error> {
                // Configure connection and enforce SQLite read-only behavior.
                configure_connection(conn, &config)?;
                conn.execute_batch("PRAGMA query_only = ON;")?;

                // Start a deferred transaction for snapshot reads.
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
                let result = f(&tx)?;
                // Commit is a no-op for read-only transactions.
                tx.commit()?;
                Ok(result)
            })
            .await
            .map_err(|e| PoolError::Interact(e.to_string()))?
            .map_err(PoolError::from);

        guard.complete_result(&result);
        result
    }

    /// Begin a pool operation: acquire an in-flight permit, check the closed flag,
    /// and run pre-operation observers. The returned guard records timing,
    /// observer completion, and telemetry outcome on `complete_result()`.
    async fn begin_op<'a>(
        &'a self,
        label: &'static str,
        kind: OperationKind,
        is_read_only: bool,
    ) -> Result<OpGuard<'a>> {
        // Acquire permit BEFORE checking closed flag to prevent race condition.
        let permit = self
            .in_flight_ops
            .acquire()
            .await
            .map_err(|_| PoolError::PoolClosed)?;

        if self.closed.load(Ordering::SeqCst) {
            return Err(PoolError::PoolClosed);
        }

        let mut context = OperationContext::for_pool_operation(label, kind, is_read_only);
        self.observers
            .before_operation(&mut context)
            .map_err(|e| PoolError::Observer(e.to_string()))?;

        Ok(OpGuard {
            _permit: permit,
            observers: &self.observers,
            context,
            span: crate::telemetry::DbSpan::pool_operation(label),
            start: Instant::now(),
        })
    }

    /// Acquire a reader connection, applying the pool-acquisition timeout.
    async fn acquire_reader(&self) -> Result<PooledConn> {
        tokio::time::timeout(self.config.pool_timeout, self.readers.get())
            .await
            .map_err(|_| PoolError::Timeout("reader pool".to_string()))?
            .map_err(|e| PoolError::PoolGet(e.to_string()))
    }

    /// Borrow the writer connection if the pool has not been consumed by `close()`.
    fn writer(&self) -> Result<&AsyncConnection> {
        self.writer.as_ref().ok_or(PoolError::PoolClosed)
    }

    /// Force a WAL checkpoint.
    ///
    /// This flushes the WAL file to the main database file.
    pub async fn checkpoint(&self) -> Result<()> {
        let guard = self
            .begin_op("pool.checkpoint", OperationKind::Other, false)
            .await?;
        let result = self.checkpoint_raw().await;
        guard.complete_result(&result);
        result
    }

    /// Execute a checkpoint without observer/lifecycle hooks.
    async fn checkpoint_raw(&self) -> Result<()> {
        self.writer()?
            .call(|conn| conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);"))
            .await
    }

    /// Get pool connection statistics.
    ///
    /// Returns information about the reader connection pool (size, available, waiting).
    /// Operation metrics/profiling are provided by configured observers.
    pub fn stats(&self) -> PoolStats {
        let status = self.readers.status();
        PoolStats {
            reader_pool_size: status.size,
            reader_pool_available: status.available,
            reader_pool_waiting: status.waiting as u32,
        }
    }

    /// Get the database file path.
    pub fn path(&self) -> &Path {
        &self.db_path
    }

    /// Close the pool gracefully.
    ///
    /// This will:
    /// 1. Prevent new operations from starting
    /// 2. Wait for all in-flight operations to complete
    /// 3. Checkpoint the WAL file
    /// 4. Stop the WAL monitor
    /// 5. Close all connections
    ///
    /// # Race Condition Prevention
    ///
    /// This method uses a semaphore to track in-flight operations and ensure
    /// graceful shutdown. The sequence is:
    /// 1. Set closed flag (prevents new operations after permit acquisition)
    /// 2. Drain semaphore (wait for all in-flight operations)
    /// 3. Proceed with cleanup
    pub async fn close(mut self) -> Result<()> {
        // 1. Prevent new operations
        self.closed.store(true, Ordering::SeqCst);

        crate::telemetry::info!(
            path = %self.db_path.display(),
            "closing database pool, waiting for in-flight operations"
        );

        // 2. Wait for all in-flight operations to complete
        // Try to unwrap the Arc - if we're the last owner, we can drain it
        // Otherwise, acquire all 1000 permits to ensure no operations are running
        let semaphore = Arc::clone(&self.in_flight_ops);
        let mut permits = Vec::new();
        for _ in 0..1000 {
            match semaphore.acquire().await {
                Ok(permit) => permits.push(permit),
                Err(_) => {
                    // Semaphore closed - this shouldn't happen, but handle it gracefully
                    crate::telemetry::warn!("semaphore closed during shutdown");
                    break;
                }
            }
        }

        crate::telemetry::info!(
            path = %self.db_path.display(),
            "all in-flight operations complete"
        );

        // 3. Checkpoint WAL before stopping monitor
        if let Err(_e) = self.checkpoint_raw().await {
            crate::telemetry::warn!(
                error = %crate::telemetry::chain(&_e),
                "checkpoint failed during shutdown"
            );
        }

        // 4. Stop WAL monitor
        if let Some(ref tx) = self.wal_shutdown {
            let _ = tx.send(true);
        }

        // 5. Close writer and surface any close error.
        if let Some(writer) = self.writer.take() {
            writer.close().await?;
        }

        // 6. Close reader pool explicitly
        self.readers.close();

        Ok(())
    }
}

fn normalize_database_path(path: &Path) -> PathBuf {
    if path == Path::new(":memory:") {
        unique_in_memory_uri()
    } else {
        path.to_path_buf()
    }
}

fn unique_in_memory_uri() -> PathBuf {
    // Shared-cache URI keeps the dedicated writer and reader pool connected to
    // the same in-memory database while preserving isolation between calls.
    let counter = IN_MEMORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "file:memdb_{}_{}?mode=memory&cache=shared",
        std::process::id(),
        counter
    ))
}

impl Drop for DatabasePool {
    fn drop(&mut self) {
        // Best-effort cleanup when pool is dropped without explicit close()
        //
        // Note: This is not the preferred way to clean up - users should call
        // close() explicitly for graceful shutdown. However, we handle drop
        // to prevent panics from tokio-rusqlite.

        // 1. Mark as closed to prevent new operations
        self.closed.store(true, Ordering::SeqCst);

        // 2. Signal WAL monitor to stop
        if let Some(ref tx) = self.wal_shutdown {
            let _ = tx.send(true);
        }

        // 3. Close reader pool (this is sync-safe)
        // Wrap in catch_unwind because deadpool-sqlite/tokio-rusqlite may panic
        // when connections are dropped without explicit close.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.readers.close();
        }));

        // 4. The writer option drops after this `Drop` impl returns. Dropping the
        // tokio-rusqlite handle closes its request channel; explicit `close()` is
        // still preferred because it can report SQLite close failures.
    }
}

/// Envelope around a single pool operation: holds the in-flight semaphore
/// permit and records timing/success to observers on completion.
///
/// The permit is released when this guard is dropped, regardless of whether
/// `complete_result()` was called — keeping shutdown semantics correct even on
/// early returns or panics.
struct OpGuard<'a> {
    _permit: SemaphorePermit<'a>,
    observers: &'a ObserverStack,
    context: OperationContext,
    span: crate::telemetry::DbSpan,
    start: Instant,
}

impl OpGuard<'_> {
    /// Record this operation's outcome to observers and telemetry.
    fn complete_result<R>(self, result: &Result<R>) {
        let duration = self.start.elapsed();
        self.observers
            .after_operation(&self.context, duration, result.is_ok());

        match result {
            Ok(_) => self.span.record_success(),
            Err(err) => self.span.record_error(err),
        }
    }

    /// Record an early operation error before the main closure runs.
    fn complete_error(self, err: &PoolError) {
        let duration = self.start.elapsed();
        self.observers
            .after_operation(&self.context, duration, false);
        self.span.record_error(err);
    }
}

/// Pool statistics for monitoring.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Current size of the reader pool.
    pub reader_pool_size: usize,
    /// Number of available reader connections.
    pub reader_pool_available: usize,
    /// Number of tasks waiting for a reader connection.
    pub reader_pool_waiting: u32,
}

#[cfg(test)]
mod tests;
