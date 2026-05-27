# sqlite-rs

[![CI](https://github.com/hotnsoursoup/sqlite-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/hotnsoursoup/sqlite-rs/actions/workflows/ci.yml)
![MSRV](https://img.shields.io/badge/MSRV-1.77-blue)

A pragmatic SQLite connection pool for Rust, with read/write split, WAL monitoring,
migrations, and an optional write queue.

Built on top of [`rusqlite`], [`tokio-rusqlite`], and [`deadpool`]. Designed for
async services that need a single embedded SQLite database to behave well under
concurrent load.

[`rusqlite`]: https://crates.io/crates/rusqlite
[`tokio-rusqlite`]: https://crates.io/crates/tokio-rusqlite
[`deadpool`]: https://crates.io/crates/deadpool

---

## Highlights

- **Read/write split** — a dedicated writer connection plus a concurrent reader pool,
  so reads don't queue behind writes.
- **WAL monitoring** — background task watches the WAL file size and warns before
  checkpoints become expensive.
- **Migrations** — semver-tagged baseline + incremental migrations with history
  tracking and recovery helpers.
- **Write queue** *(optional)* — coordinate concurrent writers with configurable
  overflow policies (block, timeout, reject, drop-oldest).
- **Batch helpers** — chunked iteration and bulk inserts with reasonable defaults.
- **Backup & integrity** — online backup, `PRAGMA integrity_check`, FK validation.
- **Observers** — one operation lifecycle for profiling, metrics, logging, and policy hooks.
- **Graceful shutdown** — drains in-flight work and checkpoints the WAL on close.

## Install

```toml
[dependencies]
sqlite-rs = { git = "https://github.com/hotnsoursoup/sqlite-rs", tag = "v0.2.0" }
tokio = { version = "1", features = ["full"] }
```

Pin to a commit for reproducibility:

```toml
sqlite-rs = { git = "https://github.com/hotnsoursoup/sqlite-rs", rev = "<commit-sha>" }
```

## Quick start

```rust
use sqlite_rs::{DatabasePool, Migration, PoolConfig};

#[tokio::main]
async fn main() -> Result<(), sqlite_rs::PoolError> {
    let pool = DatabasePool::open("data/app.db", PoolConfig::default()).await?;

    pool.migrate(&[
        Migration::baseline("1.0.0", r#"
            CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        "#),
        Migration::incremental("1.1.0", "1.0.0", r#"
            ALTER TABLE users ADD COLUMN email TEXT;
        "#).with_description("add email column"),
    ]).await?;

    // Concurrent on the reader pool
    let count: i64 = pool.read(|conn| {
        conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
    }).await?;

    // Serialized on the writer connection
    pool.write(|conn| {
        conn.execute("INSERT INTO users (name) VALUES (?)", ["Alice"])
    }).await?;

    // Transactions go through the writer
    pool.transaction(|tx| {
        tx.execute("UPDATE accounts SET balance = balance - 100 WHERE id = 1", [])?;
        tx.execute("UPDATE accounts SET balance = balance + 100 WHERE id = 2", [])?;
        Ok(())
    }).await?;

    pool.close().await?;
    Ok(())
}
```

A full runnable example lives in [`examples/basic_usage.rs`](examples/basic_usage.rs).

## Architecture

```text
DatabasePool
├── writer        single connection, serialized writes
├── readers       deadpool of N connections, concurrent reads
└── wal-monitor   background task (optional)
```

SQLite uses file-level locking. WAL mode lets readers proceed during writes, but
writers still block each other. Funnelling writes through one connection avoids
contention and the `SQLITE_BUSY` errors that come with it; readers scale out
independently.

## Configuration

```rust
use sqlite_rs::{PoolConfig, SynchronousMode, WalConfig};
use std::time::Duration;

// Built-in presets
let cfg = PoolConfig::default();      // 4 readers, WAL monitor on
let cfg = PoolConfig::production();   // larger pool, longer timeouts
let cfg = PoolConfig::minimal();      // testing / low-resource

// Builder
let cfg = PoolConfig::default()
    .with_reader_count(8)
    .with_busy_timeout(Duration::from_secs(10))
    .with_cache_size_kb(32_000)
    .without_wal_monitor();
```

Notable knobs: `reader_count`, `busy_timeout`, `pool_timeout`, `cache_size_kb`,
`synchronous` (`Normal` / `Full`), `wal` (checkpoint + warning thresholds), and
an `init_hook` for `PRAGMA` overrides per connection.

## Migrations

```rust
use sqlite_rs::Migration;

const MIGRATIONS: &[Migration] = &[
    Migration::baseline("1.0.0", include_str!("../migrations/001_initial.sql")),
    Migration::incremental("1.1.0", "1.0.0", include_str!("../migrations/002_add_email.sql"))
        .with_description("add email to users"),
    Migration::incremental("1.2.0", "1.1.0", include_str!("../migrations/003_index_posts.sql"))
        .with_description("index posts by user"),
];

pool.migrate(MIGRATIONS).await?;
```

Migrations are tracked in a `schema_migrations` table. A *baseline* applies when
the database is fresh; *incremental* migrations apply on top in version order.
Recovery helpers (`validate_schema_integrity`, `repair_database`,
`reset_database_with_options`) live in the same module for one-off ops.

## Write queue

When several tasks need to write concurrently, the writer connection becomes the
bottleneck. The optional `PoolWriteQueue` serialises writes through a bounded
channel with a chosen overflow policy.

```rust
use sqlite_rs::write_queue::{PoolWriteQueue, WriteQueueConfig, OverflowPolicy};
use std::sync::Arc;
use std::time::Duration;

let pool = Arc::new(pool);
let queue = PoolWriteQueue::new(Arc::clone(&pool), WriteQueueConfig {
    capacity: 1000,
    overflow_policy: OverflowPolicy::BlockTimeout(Duration::from_secs(5)),
});

// Fire-and-forget
queue.fire_and_forget(|conn| {
    conn.execute("INSERT INTO events (type) VALUES (?)", ["click"])?;
    Ok(())
}).await?;

// Awaitable result
let handle = queue.enqueue(|conn| {
    conn.query_row(
        "INSERT INTO users (name) VALUES (?) RETURNING id",
        ["Alice"],
        |row| row.get::<_, i64>(0),
    )
}).await?;
let user_id: i64 = handle.await?;

queue.shutdown().await;
```

| Policy             | Behaviour                          | Best for                  |
|--------------------|------------------------------------|---------------------------|
| `Block`            | Wait indefinitely                  | Batch jobs                |
| `BlockTimeout(d)`  | Wait up to `d`, then error         | General services          |
| `Reject`           | Error immediately                  | Callers with own retry    |
| `DropOldest`       | Evict oldest entry to make room    | Metrics / telemetry       |

Presets: `WriteQueueConfig::for_metrics()`, `::for_critical()`,
`::non_blocking()`.

## Monitoring

```rust
let stats = pool.stats();
println!(
    "readers: {}/{} available, {} waiting",
    stats.reader_pool_available, stats.reader_pool_size, stats.reader_pool_waiting,
);

pool.checkpoint().await?; // manual WAL checkpoint
```

For operation-level observability, attach observers to the pool config and keep
handles to the observers you want to inspect later:

```rust
use sqlite_rs::observer::{MetricsCollector, QueryLogger};
use sqlite_rs::profiling::QueryProfiler;
use sqlite_rs::PoolConfig;
use std::time::Duration;

let metrics = MetricsCollector::new();
let profiler = QueryProfiler::new(Duration::from_millis(100));

let config = PoolConfig::default()
    .with_observer(QueryLogger::new())
    .with_observer(metrics.clone())
    .with_observer(profiler.clone());

// After using a pool opened with this config:
let query_stats = profiler.stats();
let query_metrics = metrics.snapshot();
```

## SQL safety

`sqlite-rs` is intentionally close to `rusqlite`: callers still own SQL text
and should bind values through parameters. Helper APIs quote identifiers where
they construct SQL from names, and batch inserters bind row values as parameters.

Some inputs are trusted SQL fragments by design, including migration SQL,
`ColumnDef::column_type`, `ColumnDef::default`, and `IndexDef::where_clause`.
Do not pass end-user text into those fragment positions. See
[`docs/sql-safety.md`](docs/sql-safety.md) for the API-by-API boundary.

## Error handling

```rust
use sqlite_rs::PoolError;

match pool.write(|c| c.execute("INSERT INTO ...", [])).await {
    Ok(_) => {}
    Err(PoolError::Sqlite(e))   => eprintln!("sqlite: {e}"),
    Err(PoolError::PoolGet(m))  => eprintln!("pool exhausted: {m}"),
    Err(e)                      => eprintln!("{e}"),
}
```

## Cargo features

| Feature       | Default | Description                                  |
|---------------|:-------:|----------------------------------------------|
| `wal-monitor` |   yes   | Background WAL size monitoring               |
| `tracing`     |    -    | Structured logging via the [`tracing`] crate |

```toml
# Minimal build
sqlite-rs = { git = "...", default-features = false }

# With tracing
sqlite-rs = { git = "...", features = ["tracing"] }
```

[`tracing`]: https://crates.io/crates/tracing

## Modules

| Module         | Purpose                                                       |
|----------------|---------------------------------------------------------------|
| `pool`         | `DatabasePool`, `PoolStats`, read/write/transaction APIs      |
| `migrations`   | Migration runner, history, recovery                           |
| `schema`       | Introspection and safe DDL helpers                            |
| `backup`       | Online backup and integrity checks                            |
| `observer`     | Unified operation lifecycle for profiling, logging, metrics, policy hooks |
| `profiling`    | `QueryProfiler` and aggregate query stats as an observer      |
| `retry`        | Exponential backoff for transient SQLite errors               |
| `batch`        | Bulk inserts and chunked iteration                            |
| `maintenance`  | Health checks, `VACUUM`, `ANALYZE`                            |
| `write_queue`  | Optional bounded pool-backed write queue                      |

## Further reading

- [`docs/when-to-use.md`](docs/when-to-use.md) — scope, when async-wrapped
  SQLite is the right tool, and when to reach for `rusqlite` directly.
- [`docs/tuning.md`](docs/tuning.md) — what each config knob does, the
  PRAGMAs applied per connection, and workload-shaped presets.
- [`docs/sql-safety.md`](docs/sql-safety.md) — which APIs bind values, which
  quote identifiers, and which accept trusted SQL fragments.
- [`docs/migrating-to-0.2.md`](docs/migrating-to-0.2.md) — observer and
  write-queue migration notes for the breaking 0.2 API.
- [`CHANGELOG.md`](CHANGELOG.md) — public release notes.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — validation and compatibility rules.

## Minimum supported Rust version

`1.77`. Bumps are not considered breaking until 1.0. This matches the current dependency graph (`deadpool-sqlite` 0.9 requires Rust 1.77+).

## Status

Pre-1.0. The API is usable and the surface is fairly stable, but expect
occasional breaking changes on minor versions until 1.0.

## License

MIT. See [`LICENSE`](LICENSE).
