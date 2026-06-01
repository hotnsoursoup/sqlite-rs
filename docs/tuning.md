# Tuning

`sqlite-kit` ships with defaults that are reasonable for a small service. This
document explains what each knob does, when to change it, and which `PRAGMA`s
are set automatically.

## PRAGMAs applied per connection

Every connection (writer and each reader) has these `PRAGMA`s set at open
time, before any query runs:

| PRAGMA           | Value                              | Purpose                                  |
|------------------|------------------------------------|------------------------------------------|
| `journal_mode`   | `WAL` *(writer; readers inherit)*  | Concurrent reads during a write          |
| `foreign_keys`   | `ON`                               | Enforce FK constraints (off by default)  |
| `busy_timeout`   | `busy_timeout` (ms)                | Internal wait on lock contention         |
| `cache_size`     | `-cache_size_kb` (negative = KB)   | Per-connection page cache                |
| `synchronous`    | `synchronous` mode                 | Durability vs. speed tradeoff            |

Use `with_init_hook` to layer additional `PRAGMA`s, custom collations, or
scalar functions on top of these.

## The knobs

### `reader_count`

How many reader connections in the pool. Defaults to **4**.

- **Too low:** reads queue behind each other under burst load.
- **Too high:** each reader holds a private page cache (see `cache_size_kb`),
  so memory grows linearly. SQLite reads are CPU/IO-bound — past ~2× core
  count you stop getting throughput, you just pay more cache.

Rule of thumb: start at `min(8, num_cores)`. Watch
`PoolStats::reader_pool_waiting`. If it's consistently > 0, raise it.

### `busy_timeout` vs `pool_timeout`

These are easy to confuse. They live at different layers:

- **`busy_timeout`** — passed to SQLite via `PRAGMA busy_timeout`. How long
  SQLite itself waits on a locked database before returning `SQLITE_BUSY`.
  Default: **5s**.
- **`pool_timeout`** — how long this pool waits for a free reader connection
  from `deadpool` before returning `PoolError::PoolGet`. Default: **30s**.

If you see `SQLITE_BUSY` errors, raise `busy_timeout`. If you see "pool
exhausted," raise `reader_count` or `pool_timeout`. They are not
interchangeable.

### `cache_size_kb`

Per-connection page cache in kilobytes. Default: **16 MB**.

This is not a shared cache — each reader and the writer all hold their own.
A pool with 8 readers and 64 MB cache uses ~576 MB just for caches. That is
often the right call (SQLite cache hits are very cheap), but be aware of the
multiplier.

Production preset uses **64 MB per connection** with **8 readers** (~576 MB).

### `synchronous`

Durability mode. Maps directly to `PRAGMA synchronous`.

| Mode     | Behaviour                                       | When                              |
|----------|-------------------------------------------------|-----------------------------------|
| `Full`   | `fsync` after every critical write              | You'd rather lose perf than data  |
| `Normal` | `fsync` at WAL checkpoints (default)            | Most apps                         |
| `Off`    | No `fsync`. Corruption risk on power loss       | Ephemeral DBs, test fixtures      |

`Normal` is safe with WAL: a crash loses at most the last transaction that
wasn't checkpointed. `Off` is only safe if losing the database on power loss
is acceptable.

### `wal` (`WalConfig`)

When `Some(_)`, a background task polls the WAL file size.

```rust
WalConfig {
    checkpoint_threshold_mb: 10,   // log + trigger checkpoint
    warning_threshold_mb:    50,   // log warning, no action
    monitor_interval:        60s,  // poll interval
}
```

- **`checkpoint_threshold_mb`** — when the WAL exceeds this, the monitor
  initiates a passive checkpoint. SQLite would eventually do this itself
  (default 1000 pages ≈ 4 MB), but the monitor gives you a way to control
  *when* it happens, which matters for write-heavy workloads where the
  default cadence interrupts at inconvenient moments.
- **`warning_threshold_mb`** — pure observability. Logs a warning so you can
  alert before the WAL becomes a real problem (very large WAL files slow
  recovery and consume disk).
- **`monitor_interval`** — how often the background task wakes. Cheaper than
  it sounds; it's a `stat()` call.

Set `wal = None` for unit tests or when you want hands-off WAL behaviour.

### `init_hook`

Runs once per connection, after PRAGMAs are applied. Use for:

- Registering custom scalar / aggregate functions (`create_scalar_function`).
- Registering custom collations.
- Setting `PRAGMA application_id` or other one-time metadata.
- Loading an extension (if `rusqlite` is built with `load_extension`).

Avoid putting per-query state here — the hook runs at connection open, not
per call.

## Picking a preset

| Preset                  | Readers | Cache/conn | Busy | WAL monitor |
|-------------------------|--------:|-----------:|-----:|:-----------:|
| `PoolConfig::minimal`   |       1 |       2 MB |   2s | off         |
| `PoolConfig::default`   |       4 |      16 MB |   5s | on (10/50)  |
| `PoolConfig::production`|       8 |      64 MB |  10s | on (20/100) |

- **`minimal`** — tests, ephemeral DBs, CLIs that happen to be async.
- **`default`** — small services, side-project APIs, dev environments.
- **`production`** — services with sustained concurrency and enough RAM to
  pay for it.

## Workload sketches

**Read-heavy API (e.g. config service, lookup service).**
Bump `reader_count` to `2× cores`. Keep cache per reader large
(`cache_size_kb = 64_000`+) — hot-set page caching is the single biggest win.
Leave WAL on with default thresholds.

**Write-heavy ingest (events, telemetry).**
Use the write queue with `OverflowPolicy::DropOldest` or `BlockTimeout`.
Lower `checkpoint_threshold_mb` to keep WAL bounded under sustained writes.
Consider `synchronous = Normal` (the default) — `Full` will hurt here.

**Mixed transactional (e.g. small CRUD app).**
`PoolConfig::default()` is usually fine. Profile before tuning. Attach a
`QueryProfiler` as an observer and keep its handle so you can inspect slow
queries before adjusting knobs.

**Test fixtures / integration tests.**
Use `DatabasePool::open_in_memory()` or `DatabasePool::open(":memory:",
PoolConfig::minimal())`. Both routes use a unique shared-cache memory URI so
the writer and reader connections see the same schema/data. Set `synchronous =
Off` if you really want speed and don't care about durability.

## When to checkpoint manually

The WAL monitor handles most cases. Reach for `pool.checkpoint().await` when:

- You're about to do a backup and want a tight WAL.
- You're shutting down outside the normal `pool.close()` path.
- You just finished a large bulk-load and want the WAL to drain before the
  next phase.

Routine checkpoints are noise — let the monitor and SQLite handle them.

## Verifying your tuning

The library exposes enough to confirm changes are doing what you think:

```rust
use sqlite_kit::observer::MetricsCollector;
use sqlite_kit::profiling::QueryProfiler;
use sqlite_kit::PoolConfig;

let metrics = MetricsCollector::new();
let profiler = QueryProfiler::default();
let config = PoolConfig::default()
    .with_observer(metrics.clone())
    .with_observer(profiler.clone());

let pool = sqlite_kit::DatabasePool::open("data/app.db", config).await?;

let pool_stats = pool.stats();
// reader_pool_size, reader_pool_available, reader_pool_waiting

let query_stats = profiler.stats();
// total_queries, slow_queries, slow_percentage(), aggregate timing

let query_metrics = metrics.snapshot();
// success_rate(), avg_latency_ms(), counts by operation kind
```

If a knob change doesn't move one of these numbers, it isn't doing what you
think it's doing.
