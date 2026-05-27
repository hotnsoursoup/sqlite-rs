//! WAL (Write-Ahead Log) monitoring and management.
//!
//! This module provides background monitoring of SQLite's WAL file
//! to prevent unbounded growth and trigger automatic checkpoints.

mod monitor;

pub use monitor::spawn_wal_monitor;
