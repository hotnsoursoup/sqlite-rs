//! WAL file size monitoring with automatic checkpointing.

use crate::config::WalConfig;
use std::path::PathBuf;
use tokio::sync::watch;

/// Spawn a background task that monitors WAL file size.
///
/// The monitor will:
/// - Check WAL file size at configured intervals
/// - Trigger `PRAGMA wal_checkpoint(PASSIVE)` when checkpoint threshold is exceeded
/// - Log warnings when warning threshold is exceeded
///
/// # Arguments
///
/// * `db_path` - Path to the SQLite database file
/// * `shutdown` - Watch receiver for shutdown signal
/// * `config` - WAL monitoring configuration
///
/// # WAL File Location
///
/// SQLite creates the WAL file at `{db_path}-wal` (appended, not extension replaced).
/// For example: `app.db` → `app.db-wal`
pub fn spawn_wal_monitor(db_path: PathBuf, mut shutdown: watch::Receiver<bool>, config: WalConfig) {
    tokio::spawn(async move {
        // WAL file is always {db_path}-wal (appended, not extension replaced)
        let mut wal_path = db_path.as_os_str().to_owned();
        wal_path.push("-wal");
        let wal_path = PathBuf::from(wal_path);

        // Open a dedicated connection for checkpointing
        // This avoids needing to coordinate with the main writer
        let checkpoint_result = tokio_rusqlite::Connection::open(&db_path).await;
        match &checkpoint_result {
            Ok(_) => {
                crate::telemetry::debug!(path = %db_path.display(), "WAL monitor started");
            }
            #[allow(unused_variables)]
            Err(e) => {
                crate::telemetry::error!(
                    error = %crate::telemetry::chain(e),
                    "WAL monitor failed to open checkpoint connection"
                );
            }
        }
        let checkpoint_conn = checkpoint_result.ok();

        let mut interval = tokio::time::interval(config.monitor_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    check_and_checkpoint(&wal_path, &checkpoint_conn, &config).await;
                }
                _ = shutdown.changed() => {
                    crate::telemetry::debug!("WAL monitor shutting down");
                    break;
                }
            }
        }

        // Close the checkpoint connection gracefully
        if let Some(conn) = checkpoint_conn {
            let _ = conn.close().await;
        }
    });
}

/// Check WAL file size and trigger checkpoint if needed.
async fn check_and_checkpoint(
    wal_path: &PathBuf,
    checkpoint_conn: &Option<tokio_rusqlite::Connection>,
    config: &WalConfig,
) {
    let Ok(meta) = tokio::fs::metadata(wal_path).await else {
        return; // WAL file doesn't exist yet, nothing to do
    };

    let size_mb = meta.len() / (1024 * 1024);

    if size_mb > config.warning_threshold_mb {
        // Warning threshold - log and checkpoint
        crate::telemetry::warn!(
            size_mb = size_mb,
            threshold_mb = config.warning_threshold_mb,
            "WAL file exceeds warning threshold"
        );

        run_checkpoint(checkpoint_conn, "warning threshold").await;
    } else if size_mb > config.checkpoint_threshold_mb {
        // Normal checkpoint threshold
        crate::telemetry::debug!(
            size_mb = size_mb,
            threshold_mb = config.checkpoint_threshold_mb,
            "WAL file exceeds checkpoint threshold, triggering checkpoint"
        );

        run_checkpoint(checkpoint_conn, "checkpoint threshold").await;
    }
}

/// Execute a WAL checkpoint.
#[allow(unused_variables)]
async fn run_checkpoint(checkpoint_conn: &Option<tokio_rusqlite::Connection>, reason: &str) {
    let Some(conn) = checkpoint_conn else {
        return;
    };

    match conn
        .call(|c| {
            c.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
                .map_err(tokio_rusqlite::Error::Rusqlite)
        })
        .await
    {
        Ok(()) => {
            crate::telemetry::info!(reason = reason, "WAL checkpoint completed");
        }
        Err(_e) => {
            crate::telemetry::warn!(
                error = %crate::telemetry::chain(&_e),
                reason = reason,
                "WAL checkpoint failed"
            );
        }
    }
}
