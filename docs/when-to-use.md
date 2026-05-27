# When to use sqlite-rs

SQLite is a synchronous, in-process, file-backed database. There is no socket,
no remote server, no I/O wait on a third party. Wrapping it in async is a
deliberate engineering choice — useful in some contexts, dead weight in others.
This document is a scope statement: what `sqlite-rs` is for, what it isn't, and
how it relates to the alternatives.

## What async actually buys you

The wrapping is not making SQLite "faster" or magically concurrent. SQLite calls
under the hood are still synchronous file I/O. What async gets you in a tokio
runtime:

1. **The runtime stays responsive.** A synchronous `rusqlite` call from inside
   an async handler pins a tokio worker thread for the duration of the query.
   Under load, a few slow queries can starve the executor. `tokio-rusqlite`
   runs each call on a dedicated background thread per connection and hands
   back a future; the executor is free during the wait.

2. **Backpressure as ordinary async primitives.** "Wait for a free reader,"
   "wait for a slot in the write queue," "time out after N seconds" — all
   expressed with semaphores, bounded channels, and `tokio::time::timeout`.
   Threads + `Mutex` + `Condvar` can do the same, but they don't compose with
   the rest of an async program.

3. **Composition.** `tokio::select!` between a query and a shutdown signal,
   `join_all` across N concurrent reads, awaiting a query alongside a timer —
   all natural in async, fiddly otherwise.

## What async does *not* buy you

- **Parallel writes.** SQLite serializes writes at the file level. WAL mode
  lets readers proceed during writes, but two writers still contend. The
  single writer connection in this pool is structural, not a workaround.
- **More throughput than disk + `fsync` will give you.** WAL helps, batching
  helps, `synchronous = NORMAL` helps — async does not.
- **Benefit in a sync app.** If you're not on tokio, the wrapping is overhead
  with no upside. Reach for [`rusqlite`] + [`r2d2`] directly.

## When to reach for sqlite-rs

Good fit:

- An Axum / tonic / actix-web / hyper service whose handlers need to read and
  write SQLite, and you don't want to wrap every call in `spawn_blocking`.
- A background worker on tokio that needs reads to proceed while writes
  serialize.
- An app where the optional write queue (`fire_and_forget`, bounded capacity,
  overflow policies) earns its keep — telemetry sinks, event ingest,
  audit logging.

Not a good fit:

- CLIs and one-shot scripts. The runtime overhead and pool complexity isn't
  worth it for "open a file, run a few statements, exit." Use [`rusqlite`].
- Synchronous web frameworks (Rocket sync mode, anything CGI-shaped). Use
  [`rusqlite`] + [`r2d2`].
- Workloads where writes dominate and there's no way to coalesce them.
  SQLite's single-writer model is the bottleneck; a different store may be
  the right answer.
- Multi-process writers to the same database. SQLite tolerates it; this pool
  doesn't coordinate across processes. WAL has its own semantics here that
  you'll need to reason about directly.

## Comparison with alternatives

| Crate                | Driver model              | When to pick                                           |
|----------------------|---------------------------|--------------------------------------------------------|
| `rusqlite`           | Sync, single connection   | CLIs, scripts, sync apps                               |
| `rusqlite` + `r2d2`  | Sync, pooled              | Sync web frameworks, batch jobs                        |
| `sqlx` (sqlite)      | Async, query-checked      | You want compile-time query checking + multi-backend   |
| `sea-orm`            | Async ORM                 | You want an ORM with multi-backend support             |
| `sqlite-rs`          | Async, read/write split   | tokio app, want read concurrency + write coordination  |

`sqlx` and `sea-orm` are heavier and more opinionated; both are fine choices
for new code. `sqlite-rs` stays close to raw SQL and `rusqlite` idioms — you
write your queries, you get the rows back. The split pool, WAL monitor,
migrations, and write queue are the value-add.

## Non-goals

This is not, and is unlikely to become:

- A query builder or ORM.
- A multi-backend abstraction. SQLite only.
- A clustering, replication, or sync layer.
- A drop-in replacement for `rusqlite`. The async surface is the point.

[`rusqlite`]: https://crates.io/crates/rusqlite
[`r2d2`]: https://crates.io/crates/r2d2
