//! Basic usage example for sqlite-kit.
//!
//! Run with: cargo run --example basic_usage

use sqlite_kit::{DatabasePool, Migration, PoolConfig};
use std::time::Duration;

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: "1.0.0",
        from_version: "0.0.0",
        description: "Initial schema",
        sql: r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT UNIQUE,
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE posts (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id),
                title TEXT NOT NULL,
                content TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE INDEX idx_posts_user ON posts(user_id);
        "#,
        kind: sqlite_kit::MigrationKind::Baseline,
        creates_tables: &["users", "posts"],
    },
    Migration {
        version: "1.1.0",
        from_version: "1.0.0",
        description: "Add user status",
        sql: "ALTER TABLE users ADD COLUMN status TEXT DEFAULT 'active';",
        kind: sqlite_kit::MigrationKind::Incremental,
        creates_tables: &[],
    },
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary database for this example
    let temp_dir = std::env::temp_dir().join("sqlite-kit-example");
    std::fs::create_dir_all(&temp_dir)?;
    let db_path = temp_dir.join("example.db");

    println!("Database path: {}", db_path.display());

    // Open pool with custom config
    let config = PoolConfig::default()
        .with_reader_count(4)
        .with_busy_timeout(Duration::from_secs(10))
        .without_wal_monitor(); // Disable for this example

    let pool = DatabasePool::open(&db_path, config).await?;
    println!("Pool opened successfully");

    // Run migrations
    pool.migrate(MIGRATIONS).await?;
    println!("Migrations applied");

    // Insert some data
    pool.write(|conn| {
        conn.execute(
            "INSERT INTO users (name, email) VALUES (?1, ?2)",
            ["Alice", "alice@example.com"],
        )?;
        conn.execute(
            "INSERT INTO users (name, email) VALUES (?1, ?2)",
            ["Bob", "bob@example.com"],
        )?;
        Ok(())
    })
    .await?;
    println!("Users inserted");

    // Insert posts in a transaction
    pool.transaction(|tx| {
        tx.execute(
            "INSERT INTO posts (user_id, title, content) VALUES (1, 'Hello World', 'My first post')",
            [],
        )?;
        tx.execute(
            "INSERT INTO posts (user_id, title, content) VALUES (1, 'Rust is Great', 'Learning sqlite-kit')",
            [],
        )?;
        tx.execute(
            "INSERT INTO posts (user_id, title, content) VALUES (2, 'Bob''s Post', 'Just saying hi')",
            [],
        )?;
        Ok(())
    })
    .await?;
    println!("Posts inserted via transaction");

    // Read data (concurrent reads)
    let user_count: i64 = pool
        .read(|conn| conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0)))
        .await?;
    println!("User count: {}", user_count);

    // Query with join
    let posts: Vec<(String, String, String)> = pool
        .read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT u.name, p.title, p.content
                 FROM posts p
                 JOIN users u ON p.user_id = u.id
                 ORDER BY p.created_at",
            )?;

            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rows)
        })
        .await?;

    println!("\nPosts:");
    for (author, title, content) in &posts {
        println!("  - {} by {}: {}", title, author, content);
    }

    // Check pool stats
    let stats = pool.stats();
    println!(
        "\nPool stats: {} readers, {} available",
        stats.reader_pool_size, stats.reader_pool_available
    );

    // Clean shutdown
    pool.close().await?;
    println!("Pool closed gracefully");

    // Cleanup (may fail on Windows due to file locking - that's ok)
    std::fs::remove_file(&db_path).ok();
    std::fs::remove_file(db_path.with_extension("db-wal")).ok();
    std::fs::remove_file(db_path.with_extension("db-shm")).ok();
    println!("Done!");

    Ok(())
}
