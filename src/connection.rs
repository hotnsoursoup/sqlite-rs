//! Async connection wrapper for SQLite.

use crate::config::{InitHook, PoolConfig};
use crate::error::{PoolError, Result};
use std::path::Path;
use std::time::Duration;
use tokio_rusqlite::Connection;

/// Async wrapper around a SQLite connection.
///
/// This provides a safe async interface to SQLite operations using tokio_rusqlite.
///
/// # Drop Behavior
///
/// For deterministic shutdown, call [`close`](Self::close). If the wrapper is
/// dropped without `close`, the underlying background worker exits when its
/// request channel is dropped.
pub struct AsyncConnection {
    conn: Connection,
}

impl AsyncConnection {
    /// Open a new async connection to the database.
    pub async fn open<P: AsRef<Path>>(path: P, config: &PoolConfig) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let conn = Connection::open(&path)
            .await
            .map_err(|e| PoolError::Open(e.to_string()))?;

        let config = config.clone();

        conn.call(move |conn| {
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
            configure_connection(conn, &config)?;
            apply_init_hook(conn, &config.init_hook)?;
            Ok(())
        })
        .await
        .map_err(|e| PoolError::Config(e.to_string()))?;

        Ok(Self { conn })
    }

    /// Execute a function on the connection.
    ///
    /// The function receives a mutable reference to the underlying rusqlite Connection
    /// and can perform any database operations.
    pub async fn call<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> std::result::Result<R, rusqlite::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.conn
            .call(move |conn| f(conn).map_err(tokio_rusqlite::Error::Rusqlite))
            .await
            .map_err(PoolError::from)
    }

    /// Execute a transaction on the connection.
    ///
    /// The transaction is automatically committed if the function returns Ok,
    /// or rolled back if it returns Err.
    pub async fn transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> std::result::Result<R, rusqlite::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.conn
            .call(move |conn| {
                let tx = conn.transaction()?;
                let result = f(&tx)?;
                tx.commit()?;
                Ok(result)
            })
            .await
            .map_err(PoolError::from)
    }

    /// Close the connection gracefully.
    ///
    /// This is the preferred way to surface SQLite close errors. Dropping the
    /// connection without calling this still releases the background worker, but
    /// cannot report close failures.
    pub async fn close(self) -> Result<()> {
        self.conn
            .close()
            .await
            .map_err(|e| PoolError::Interact(e.to_string()))
    }
}

/// Configure a raw rusqlite connection with standard settings.
///
/// This is useful when you need to configure connections obtained from a pool.
/// This does not apply the init hook; hooks are connection-lifetime setup and
/// should be applied once when the connection is created.
pub fn configure_connection(
    conn: &rusqlite::Connection,
    config: &PoolConfig,
) -> std::result::Result<(), rusqlite::Error> {
    let busy_timeout_ms = busy_timeout_millis(config.busy_timeout);
    let cache_size_kb = config.cache_size_kb;
    let sync_mode = config.synchronous.as_pragma_value();

    conn.execute_batch(&format!(
        "PRAGMA busy_timeout = {};
         PRAGMA foreign_keys = ON;
         PRAGMA cache_size = -{};
         PRAGMA synchronous = {};",
        busy_timeout_ms, cache_size_kb, sync_mode
    ))?;

    Ok(())
}

fn busy_timeout_millis(timeout: Duration) -> i32 {
    timeout.as_millis().min(i32::MAX as u128) as i32
}

/// Apply just the init hook to a connection.
///
/// Useful when the connection is already configured but you need to apply
/// the init hook (e.g., on reader pool connections).
pub fn apply_init_hook(
    conn: &rusqlite::Connection,
    hook: &Option<InitHook>,
) -> std::result::Result<(), rusqlite::Error> {
    if let Some(ref hook) = hook {
        hook(conn)?;
    }
    Ok(())
}
