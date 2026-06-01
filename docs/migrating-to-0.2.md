# Migrating to 0.2

`0.2.0` intentionally removes the parallel middleware/profiler surfaces and the
standalone write queue. The public API now has one observability model and one
write-queue composition root.

## Observer migration

### Before

```rust,ignore
let config = PoolConfig {
    profiler: Some(QueryProfiler::default()),
    ..PoolConfig::default()
};

let pool = DatabasePool::open("data/app.db", config).await?;
let stats = pool.query_stats();
```

or a middleware wrapper around the pool:

```rust,ignore
let pool = DatabasePool::open("data/app.db", PoolConfig::default()).await?;
let pool = MiddlewarePool::new(pool).with(QueryLogger::new());
```

### After

Attach observers to `PoolConfig`. Keep cloned handles to the observers you want
to inspect later.

```rust
use sqlite_kit::observer::{MetricsCollector, QueryLogger};
use sqlite_kit::profiling::QueryProfiler;
use sqlite_kit::{DatabasePool, PoolConfig};
use std::time::Duration;

# async fn example() -> Result<(), sqlite_kit::PoolError> {
let metrics = MetricsCollector::new();
let profiler = QueryProfiler::new(Duration::from_millis(100));

let config = PoolConfig::default()
    .with_observer(QueryLogger::new())
    .with_observer(metrics.clone())
    .with_observer(profiler.clone());

let pool = DatabasePool::open("data/app.db", config).await?;

let pool_stats = pool.stats();
let query_stats = profiler.stats();
let query_metrics = metrics.snapshot();
# let _ = (pool_stats, query_stats, query_metrics);
# Ok(())
# }
```

## Custom observer migration

Implement `observer::Observer` instead of the old middleware trait. Use
`before_operation` to reject work before SQLite is touched, and
`after_operation` for non-failing metrics/logging.

```rust
use sqlite_kit::observer::{Observer, ObserverError, ObserverResult, OperationContext};
use std::time::Duration;

struct ReadOnlyGate;

impl Observer for ReadOnlyGate {
    fn before_operation(&self, ctx: &mut OperationContext) -> ObserverResult<()> {
        if !ctx.is_read_only() {
            return Err(ObserverError::Blocked {
                reason: "writes are disabled in this process".to_string(),
            });
        }
        Ok(())
    }

    fn after_operation(&self, _ctx: &OperationContext, _duration: Duration, _success: bool) {}
}
```

## Write queue migration

The standalone `WriteQueue` was removed because it required callers to supply a
separate `AsyncConnection`, creating a second write composition root. Use
`PoolWriteQueue` so queued writes share the same authoritative pool writer.

### Before

```rust,ignore
let conn = AsyncConnection::open("data/app.db", &PoolConfig::default()).await?;
let queue = WriteQueue::new(conn, WriteQueueConfig::default());
```

### After

```rust
use sqlite_kit::write_queue::{PoolWriteQueue, WriteQueueConfig};
use sqlite_kit::{DatabasePool, PoolConfig};
use std::sync::Arc;

# async fn example() -> Result<(), sqlite_kit::PoolError> {
let pool = Arc::new(DatabasePool::open("data/app.db", PoolConfig::default()).await?);
let queue = PoolWriteQueue::new(Arc::clone(&pool), WriteQueueConfig::default());

queue.fire_and_forget(|conn| {
    conn.execute("INSERT INTO events (kind) VALUES (?1)", ["click"])?;
    Ok(())
}).await?;
# Ok(())
# }
```
