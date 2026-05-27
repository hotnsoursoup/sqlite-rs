# Changelog

All notable changes to this project will be documented here.

This crate is pre-1.0. Minor-version releases may include breaking API changes;
patch releases should remain source-compatible unless a security or soundness fix
requires otherwise.

## Unreleased

No unreleased changes.

## 0.2.0 - 2026-05-25

### Breaking

- Replaced the separate middleware/profiler pool integration with a single
  `observer` module. Attach `QueryLogger`, `MetricsCollector`, `QueryProfiler`,
  or custom `Observer` implementations through `PoolConfig::with_observer`.
- Removed `PoolConfig::profiler`, `DatabasePool::query_stats()`, and
  `DatabasePool::profiler()`. Keep a cloned observer/profiler handle when code
  needs to inspect metrics after pool operations.
- Removed the standalone `WriteQueue` and its direct `AsyncConnection` surface.
  Use `PoolWriteQueue` so all queued writes go through the pool's authoritative
  writer connection.
- Renamed the old `middleware` module to `observer` and removed
  `MiddlewarePool`; operation interception now lives in `DatabasePool` itself.

### Added

- Added GitHub Actions CI gates for formatting, clippy, tests, docs,
  no-default-features, and MSRV checks.
- Added `docs/sql-safety.md` to document which APIs bind values, which quote
  identifiers, and which intentionally accept trusted SQL fragments.
- Added a unified `observer` module with `Observer`, `ObserverStack`,
  `QueryLogger`, and `MetricsCollector`.

### Changed

- Bumped crate version to `0.2.0` for the observer/write-queue breaking pass.
- Raised the stated MSRV to Rust 1.77 to match the dependency graph.
- Capped the `tempfile` dev dependency below 3.25 so the test suite remains
  compatible with the stated MSRV.
- `QueryProfiler` now implements `observer::Observer`, so profiling, metrics,
  logging, and policy rejection share the same operation lifecycle.
- `PoolWriteQueue` remains the only public write queue, backed by the shared
  bounded queue primitive and the pool writer.
- Refactored telemetry, pool tests, migration recovery, migration runner, and
  batch inserter tests into smaller files without public API changes.
- Replaced the leak-on-drop `AsyncConnection`/pool writer wrapper with normal
  handle ownership. Explicit `close()` still reports SQLite close failures;
  dropping now lets the background worker exit instead of intentionally leaking.

### Fixed

- Fixed `DatabasePool::open(":memory:", ...)` and `open_in_memory()` so the
  writer and reader connections share the same in-memory database instead of
  observing separate SQLite private in-memory databases.
- Fixed `PoolWriteQueue` worker wakeups so tasks enqueued before the worker
  parks are drained without waiting for a second notification.
- Fixed `PoolWriteQueue` error statistics for awaitable writes that fail inside
  the queued operation.
- Fixed write-queue worker shutdown when a queue is dropped without explicit
  `shutdown()`, and reject zero-capacity configs constructed manually.
- Fixed `MigrationOptions::default()` to match `MigrationOptions::new()` and
  preserve create-table validation by default.
- Hardened `BatchInserter` and `TypedBatchInserter` to quote table/column
  identifiers, support schema-qualified table names, reject malformed table
  identifiers, reject empty column sets, reject row-width mismatches, and reject
  `rows_per_statement = 0` before chunking.
- Fixed bounded write-queue capacity accounting and shutdown wakeups around
  `DropOldest`, in-flight work, and blocked producers.
- Wired pool operation spans into the existing operation guard under the tracing
  feature.

## 0.1.0

- Initial public pre-release surface: async SQLite pool, read/write split, WAL
  monitoring, migrations, write queues, backup/integrity helpers, schema helpers,
  batch helpers, retry utilities, observers/profiling, and savepoints.
