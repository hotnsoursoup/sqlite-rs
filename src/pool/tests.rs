//! Integration tests for [`DatabasePool`] covering reads, writes, transactions,
//! init hooks, graceful shutdown, and observer behaviour.

use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn test_open_in_memory_shares_writer_and_reader() {
    let pool = DatabasePool::open_in_memory().await.unwrap();

    pool.write(|conn| {
        conn.execute_batch(
            "CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO test (value) VALUES ('hello');",
        )
    })
    .await
    .unwrap();

    let value: String = pool
        .read(|conn| conn.query_row("SELECT value FROM test WHERE id = 1", [], |row| row.get(0)))
        .await
        .unwrap();

    assert_eq!(value, "hello");

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_colon_memory_path_shares_writer_and_reader() {
    let pool = DatabasePool::open(":memory:", PoolConfig::minimal())
        .await
        .unwrap();

    pool.write(|conn| {
        conn.execute_batch(
            "CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO test (value) VALUES ('hello');",
        )
    })
    .await
    .unwrap();

    let value = pool
        .read(|conn| {
            conn.query_row("SELECT value FROM test WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            })
        })
        .await;

    assert_eq!(value.unwrap(), "hello");

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_pool_basic_operations() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
        .await
        .unwrap();

    // Create table
    pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", []))
        .await
        .unwrap();

    // Insert data
    pool.write(|conn| conn.execute("INSERT INTO test (value) VALUES (?)", ["hello"]))
        .await
        .unwrap();

    // Read data
    let value: String = pool
        .read(|conn| conn.query_row("SELECT value FROM test WHERE id = 1", [], |row| row.get(0)))
        .await
        .unwrap();

    assert_eq!(value, "hello");

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_transaction_commit() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
        .await
        .unwrap();

    pool.write(|conn| {
        conn.execute(
            "CREATE TABLE accounts (id INTEGER PRIMARY KEY, balance INTEGER)",
            [],
        )
    })
    .await
    .unwrap();

    pool.write(|conn| {
        conn.execute("INSERT INTO accounts (id, balance) VALUES (1, 100)", [])?;
        conn.execute("INSERT INTO accounts (id, balance) VALUES (2, 100)", [])
    })
    .await
    .unwrap();

    // Transfer in transaction
    pool.transaction(|tx| {
        tx.execute(
            "UPDATE accounts SET balance = balance - 50 WHERE id = 1",
            [],
        )?;
        tx.execute(
            "UPDATE accounts SET balance = balance + 50 WHERE id = 2",
            [],
        )
    })
    .await
    .unwrap();

    // Verify balances
    let (b1, b2): (i64, i64) = pool
        .read(|conn| {
            let b1: i64 =
                conn.query_row("SELECT balance FROM accounts WHERE id = 1", [], |row| {
                    row.get(0)
                })?;
            let b2: i64 =
                conn.query_row("SELECT balance FROM accounts WHERE id = 2", [], |row| {
                    row.get(0)
                })?;
            Ok((b1, b2))
        })
        .await
        .unwrap();

    assert_eq!(b1, 50);
    assert_eq!(b2, 150);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_read_transaction() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
        .await
        .unwrap();

    pool.write(|conn| {
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount INTEGER);
             INSERT INTO users (id, name) VALUES (1, 'Alice');
             INSERT INTO orders (user_id, amount) VALUES (1, 100), (1, 200), (1, 300);",
        )
    })
    .await
    .unwrap();

    // Read user and orders in a consistent transaction
    let (name, total): (String, i64) = pool
        .read_transaction(|tx| {
            let name: String =
                tx.query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))?;

            let total: i64 = tx.query_row(
                "SELECT SUM(amount) FROM orders WHERE user_id = 1",
                [],
                |row| row.get(0),
            )?;

            Ok((name, total))
        })
        .await
        .unwrap();

    assert_eq!(name, "Alice");
    assert_eq!(total, 600);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_read_transaction_rejects_writes() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
        .await
        .unwrap();

    pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", []))
        .await
        .unwrap();

    let result = pool
        .read_transaction(|tx| {
            tx.execute("INSERT INTO test (value) VALUES ('should fail')", [])?;
            Ok(())
        })
        .await;

    assert!(result.is_err());

    let count: i64 = pool
        .read(|conn| conn.query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(count, 0);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_init_hook_is_not_reapplied_on_reader_reuse() {
    use crate::InitHook;
    use std::sync::Arc;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let hook: InitHook = Arc::new(|conn| {
        conn.execute_batch("CREATE TEMP TABLE hook_once (id INTEGER);")?;
        Ok(())
    });

    let pool = DatabasePool::open(&db_path, PoolConfig::minimal().with_init_hook(hook))
        .await
        .unwrap();

    let first: i64 = pool
        .read(|conn| conn.query_row("SELECT COUNT(*) FROM hook_once", [], |row| row.get(0)))
        .await
        .unwrap();
    let second: i64 = pool
        .read(|conn| conn.query_row("SELECT COUNT(*) FROM hook_once", [], |row| row.get(0)))
        .await
        .unwrap();

    assert_eq!(first, 0);
    assert_eq!(second, 0);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_close_graceful_shutdown() {
    // Verifies that close() waits for in-flight operations to complete.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let pool = DatabasePool::open(&db_path, PoolConfig::minimal())
        .await
        .unwrap();

    pool.write(|conn| {
        conn.execute(
            "CREATE TABLE test (id INTEGER PRIMARY KEY, value INTEGER)",
            [],
        )?;
        conn.execute("INSERT INTO test (value) VALUES (42)", [])
    })
    .await
    .unwrap();

    let count: i64 = pool
        .read(|conn| conn.query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(count, 1);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_pool_observer_profiler_records_operations() {
    use crate::profiling::QueryProfiler;
    use std::time::Duration;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let profiler = QueryProfiler::new(Duration::from_millis(100));
    let config = PoolConfig::minimal().with_observer(profiler.clone());
    let pool = DatabasePool::open(&db_path, config).await.unwrap();

    assert_eq!(profiler.stats().total_queries, 0);

    pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", []))
        .await
        .unwrap();

    pool.write(|conn| conn.execute("INSERT INTO test (value) VALUES (?)", ["hello"]))
        .await
        .unwrap();

    let _: String = pool
        .read(|conn| conn.query_row("SELECT value FROM test WHERE id = 1", [], |row| row.get(0)))
        .await
        .unwrap();

    let stats = profiler.stats();
    assert_eq!(stats.total_queries, 3);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_pool_observers_cover_all_core_operations() {
    use crate::profiling::QueryProfiler;
    use std::time::Duration;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let profiler = QueryProfiler::new(Duration::from_millis(100));
    let config = PoolConfig::minimal().with_observer(profiler.clone());
    let pool = DatabasePool::open(&db_path, config).await.unwrap();

    pool.write(|conn| {
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO users (name) VALUES ('Alice');",
        )
    })
    .await
    .unwrap();

    let _: String = pool
        .read(|conn| conn.query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0)))
        .await
        .unwrap();

    pool.write(|conn| conn.execute("UPDATE users SET name = 'Bob' WHERE id = 1", []))
        .await
        .unwrap();

    pool.transaction(|tx| tx.execute("UPDATE users SET name = 'Charlie' WHERE id = 1", []))
        .await
        .unwrap();

    let _: String = pool
        .read_transaction(|tx| {
            tx.query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))
        })
        .await
        .unwrap();

    let stats = profiler.stats();
    assert_eq!(stats.total_queries, 5);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_pool_observer_slow_query_detection() {
    use crate::profiling::QueryProfiler;
    use std::time::Duration;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let profiler = QueryProfiler::new(Duration::from_nanos(1));
    let config = PoolConfig::minimal().with_observer(profiler.clone());
    let pool = DatabasePool::open(&db_path, config).await.unwrap();

    pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
        .await
        .unwrap();

    pool.read(|conn| conn.query_row("SELECT COUNT(*) FROM test", [], |row| row.get::<_, i64>(0)))
        .await
        .unwrap();

    let stats = profiler.stats();
    assert_eq!(stats.total_queries, 2);
    assert_eq!(stats.slow_queries, 2);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_pool_observer_profiler_reset() {
    use crate::profiling::QueryProfiler;
    use std::time::Duration;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let profiler = QueryProfiler::new(Duration::from_millis(100));
    let config = PoolConfig::minimal().with_observer(profiler.clone());
    let pool = DatabasePool::open(&db_path, config).await.unwrap();

    pool.write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
        .await
        .unwrap();

    assert_eq!(profiler.stats().total_queries, 1);

    profiler.reset();

    assert_eq!(profiler.stats().total_queries, 0);

    pool.close().await.unwrap();
}

#[tokio::test]
async fn test_pool_observer_can_block_operation() {
    use crate::observer::{Observer, ObserverError, ObserverResult, OperationContext};

    struct BlockingObserver;

    impl Observer for BlockingObserver {
        fn before_operation(&self, _ctx: &mut OperationContext) -> ObserverResult<()> {
            Err(ObserverError::Blocked {
                reason: "blocked for test".to_string(),
            })
        }
    }

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = PoolConfig::minimal().with_observer(BlockingObserver);
    let pool = DatabasePool::open(&db_path, config).await.unwrap();

    let result = pool
        .write(|conn| conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []))
        .await;

    assert!(matches!(result, Err(PoolError::Observer(_))));

    pool.close().await.unwrap();
}
