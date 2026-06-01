//! Configuration types for the connection pool.

use crate::observer::{Observer, ObserverStack};
use std::sync::Arc;
use std::time::Duration;

/// Initialization hook for custom connection setup.
///
/// This hook is called after the connection is opened and configured with
/// standard pragmas, but before it's returned for use. Use this to register
/// custom SQL functions, collations, or other connection-level setup.
///
/// # Example
///
/// ```rust,no_run
/// use sqlite_kit::{PoolConfig, InitHook};
/// use std::sync::Arc;
///
/// // Register custom setup (e.g., application_id)
/// let hook: InitHook = Arc::new(|conn| {
///     conn.execute("PRAGMA application_id = 12345", [])?;
///     Ok(())
/// });
///
/// let config = PoolConfig::default().with_init_hook(hook);
/// ```
///
/// For registering custom SQL functions (requires rusqlite `functions` feature):
///
/// ```rust,ignore
/// use sqlite_kit::{PoolConfig, InitHook};
/// use std::sync::Arc;
///
/// let hook: InitHook = Arc::new(|conn| {
///     conn.create_scalar_function("my_hash", 1, rusqlite::functions::FunctionFlags::DETERMINISTIC, |ctx| {
///         let value: String = ctx.get(0)?;
///         Ok(format!("hashed_{}", value))
///     })?;
///     Ok(())
/// });
/// ```
pub type InitHook = Arc<dyn Fn(&rusqlite::Connection) -> Result<(), rusqlite::Error> + Send + Sync>;

/// Configuration for the database connection pool.
#[derive(Clone)]
pub struct PoolConfig {
    /// Number of reader connections in the pool.
    /// Default: 4
    pub reader_count: usize,

    /// Busy timeout for SQLite operations.
    /// Default: 5 seconds
    pub busy_timeout: Duration,

    /// Timeout for acquiring a connection from the pool.
    /// Default: 30 seconds
    pub pool_timeout: Duration,

    /// SQLite cache size in KB per connection.
    /// Default: 16000 (16MB)
    pub cache_size_kb: u32,

    /// SQLite synchronous mode.
    /// Default: "NORMAL" (good balance of safety and performance)
    pub synchronous: SynchronousMode,

    /// WAL monitoring configuration.
    /// Set to None to disable WAL monitoring.
    pub wal: Option<WalConfig>,

    /// Initialization hook called after connection is configured.
    /// Use this to register custom SQL functions, collations, etc.
    pub init_hook: Option<InitHook>,

    /// Operation observers for logging, metrics, profiling, and policy checks.
    ///
    /// Observers run for pool operations such as read, write, transactions,
    /// migrations, and manual checkpoints.
    pub observers: ObserverStack,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("reader_count", &self.reader_count)
            .field("busy_timeout", &self.busy_timeout)
            .field("pool_timeout", &self.pool_timeout)
            .field("cache_size_kb", &self.cache_size_kb)
            .field("synchronous", &self.synchronous)
            .field("wal", &self.wal)
            .field("init_hook", &self.init_hook.as_ref().map(|_| "<fn>"))
            .field("observers", &self.observers.names())
            .finish()
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            reader_count: 4,
            busy_timeout: Duration::from_secs(5),
            pool_timeout: Duration::from_secs(30),
            cache_size_kb: 16_000,
            synchronous: SynchronousMode::Normal,
            wal: Some(WalConfig::default()),
            init_hook: None,
            observers: ObserverStack::new(),
        }
    }
}

impl PoolConfig {
    /// Create a minimal config for testing or low-resource environments.
    pub fn minimal() -> Self {
        Self {
            reader_count: 1,
            busy_timeout: Duration::from_secs(2),
            pool_timeout: Duration::from_secs(5),
            cache_size_kb: 2_000,
            synchronous: SynchronousMode::Normal,
            wal: None,
            init_hook: None,
            observers: ObserverStack::new(),
        }
    }

    /// Create a high-performance config for production workloads.
    pub fn production() -> Self {
        Self {
            reader_count: 8,
            busy_timeout: Duration::from_secs(10),
            pool_timeout: Duration::from_secs(30),
            cache_size_kb: 64_000,
            synchronous: SynchronousMode::Normal,
            wal: Some(WalConfig {
                checkpoint_threshold_mb: 20,
                warning_threshold_mb: 100,
                monitor_interval: Duration::from_secs(30),
            }),
            init_hook: None,
            observers: ObserverStack::new(),
        }
    }

    /// Builder method to set reader count.
    pub fn with_reader_count(mut self, count: usize) -> Self {
        self.reader_count = count.max(1);
        self
    }

    /// Builder method to set busy timeout.
    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }

    /// Builder method to set cache size in kilobytes.
    pub fn with_cache_size_kb(mut self, size: u32) -> Self {
        self.cache_size_kb = size;
        self
    }

    /// Builder method to disable WAL monitoring.
    pub fn without_wal_monitor(mut self) -> Self {
        self.wal = None;
        self
    }

    /// Builder method to set initialization hook.
    ///
    /// The hook is called after the connection is opened and configured,
    /// allowing you to register custom SQL functions, collations, etc.
    pub fn with_init_hook(mut self, hook: InitHook) -> Self {
        self.init_hook = Some(hook);
        self
    }

    /// Builder method to add an operation observer.
    ///
    /// Observers run directly from [`DatabasePool`](crate::DatabasePool) for
    /// read/write operations, transactions, migrations, and manual checkpoints.
    pub fn with_observer<O: Observer + 'static>(mut self, observer: O) -> Self {
        self.observers.push(observer);
        self
    }

    /// Builder method to add a shared operation observer.
    pub fn with_observer_arc(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observers.push_arc(observer);
        self
    }

    /// Builder method to replace the observer stack.
    pub fn with_observers(mut self, observers: ObserverStack) -> Self {
        self.observers = observers;
        self
    }
}

/// SQLite synchronous mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynchronousMode {
    /// Sync after each critical disk operation (slowest, safest)
    Full,
    /// Sync at critical moments (good balance) - recommended
    Normal,
    /// No sync (fastest, risk of corruption on power loss)
    Off,
}

impl SynchronousMode {
    pub fn as_pragma_value(&self) -> &'static str {
        match self {
            SynchronousMode::Full => "FULL",
            SynchronousMode::Normal => "NORMAL",
            SynchronousMode::Off => "OFF",
        }
    }
}

/// Configuration for WAL (Write-Ahead Log) monitoring.
#[derive(Debug, Clone)]
pub struct WalConfig {
    /// Trigger checkpoint when WAL exceeds this size (in MB).
    /// Default: 10MB
    pub checkpoint_threshold_mb: u64,

    /// Log warning when WAL exceeds this size (in MB).
    /// Default: 50MB
    pub warning_threshold_mb: u64,

    /// How often to check WAL file size.
    /// Default: 60 seconds
    pub monitor_interval: Duration,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            checkpoint_threshold_mb: 10,
            warning_threshold_mb: 50,
            monitor_interval: Duration::from_secs(60),
        }
    }
}
