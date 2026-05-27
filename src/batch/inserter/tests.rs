//! Tests for `BatchInserter` and `TypedBatchInserter`.

use super::*;
use rusqlite::Connection;

#[test]
fn test_batch_inserter() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)",
        [],
    )
    .unwrap();

    let mut inserter = BatchInserter::new("users", &["name", "email"]);
    inserter.add(&["Alice", "alice@example.com"]);
    inserter.add(&["Bob", "bob@example.com"]);
    inserter.add(&["Charlie", "charlie@example.com"]);

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 3);

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 3);
}

#[test]
fn test_batch_inserter_quotes_identifiers() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE \"order\" (\"select\" TEXT, \"with space\" TEXT)",
        [],
    )
    .unwrap();

    let mut inserter = BatchInserter::new("order", &["select", "with space"]);
    inserter.add(&["one", "two"]);

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 1);

    let value: String = conn
        .query_row("SELECT \"with space\" FROM \"order\"", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "two");
}

#[test]
fn test_batch_inserter_quotes_schema_qualified_table() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE users (name TEXT)", []).unwrap();

    let mut inserter = BatchInserter::new("main.users", &["name"]);
    inserter.add(&["Alice"]);

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 1);

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 1);
}

#[test]
fn test_batch_inserter_rejects_zero_rows_per_statement() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE users (name TEXT)", []).unwrap();

    let mut inserter = BatchInserter::new("users", &["name"]).with_config(BatchConfig {
        rows_per_statement: 0,
        use_transaction: true,
    });
    inserter.add(&["Alice"]);

    let err = inserter.execute(&mut conn).unwrap_err();
    assert!(err.to_string().contains("rows_per_statement"));
    assert_eq!(inserter.len(), 1);
}

#[test]
fn test_batch_inserter_rejects_row_shape_mismatch() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE users (name TEXT, email TEXT)", [])
        .unwrap();

    let mut inserter = BatchInserter::new("users", &["name", "email"]);
    inserter.add(&["Alice"]);

    let err = inserter.execute(&mut conn).unwrap_err();
    assert!(err.to_string().contains("expected 2"));
    assert_eq!(inserter.len(), 1);

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 0);
}

#[test]
fn test_upsert_replace() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT UNIQUE, email TEXT)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO users (name, email) VALUES ('Alice', 'old@example.com')",
        [],
    )
    .unwrap();

    let mut inserter =
        BatchInserter::new("users", &["name", "email"]).with_upsert_mode(UpsertMode::Replace);
    inserter.add(&["Alice", "new@example.com"]);
    inserter.execute(&mut conn).unwrap();

    let email: String = conn
        .query_row("SELECT email FROM users WHERE name = 'Alice'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(email, "new@example.com");
}

#[test]
fn test_upsert_ignore() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT UNIQUE, email TEXT)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO users (name, email) VALUES ('Alice', 'old@example.com')",
        [],
    )
    .unwrap();

    let mut inserter =
        BatchInserter::new("users", &["name", "email"]).with_upsert_mode(UpsertMode::Ignore);
    inserter.add(&["Alice", "new@example.com"]);
    inserter.add(&["Bob", "bob@example.com"]);
    inserter.execute(&mut conn).unwrap();

    // Alice keeps old email
    let email: String = conn
        .query_row("SELECT email FROM users WHERE name = 'Alice'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(email, "old@example.com");

    // Bob inserted
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 2);
}

#[test]
fn test_large_batch_chunking() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE data (id INTEGER PRIMARY KEY, value TEXT)", [])
        .unwrap();

    let mut inserter = BatchInserter::new("data", &["value"]).with_config(BatchConfig {
        rows_per_statement: 10,
        use_transaction: true,
    });

    for i in 0..25 {
        inserter.add(&[format!("value_{}", i)]);
    }

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 25);

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM data", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 25);
}

#[test]
fn test_typed_batch_inserter() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price REAL, stock INTEGER)",
        [],
    )
    .unwrap();

    let mut inserter = TypedBatchInserter::new("products", &["name", "price", "stock"]);

    inserter.add_row(&[
        Value::Text("Widget".to_string()),
        Value::Real(29.99),
        Value::Integer(100),
    ]);

    inserter.add_row(&[
        Value::Text("Gadget".to_string()),
        Value::Real(49.99),
        Value::Integer(50),
    ]);

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 2);

    let (name, price, stock): (String, f64, i64) = conn
        .query_row(
            "SELECT name, price, stock FROM products WHERE name = 'Widget'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(name, "Widget");
    assert!((price - 29.99).abs() < 0.001);
    assert_eq!(stock, 100);
}

#[test]
fn test_typed_batch_row_builder() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE items (name TEXT, count INTEGER, weight REAL, data BLOB, note TEXT)",
        [],
    )
    .unwrap();

    let mut inserter =
        TypedBatchInserter::new("items", &["name", "count", "weight", "data", "note"]);

    inserter
        .row()
        .text("Item1")
        .integer(10)
        .real(1.5)
        .blob(vec![1, 2, 3])
        .null()
        .finish();

    inserter
        .row()
        .text("Item2")
        .integer(20)
        .real(2.5)
        .blob(vec![4, 5, 6])
        .text("has note")
        .finish();

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 2);

    // NULL handling
    let note: Option<String> = conn
        .query_row("SELECT note FROM items WHERE name = 'Item1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(note.is_none());

    // Blob round-trip
    let data: Vec<u8> = conn
        .query_row("SELECT data FROM items WHERE name = 'Item1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(data, vec![1, 2, 3]);
}

#[test]
fn test_typed_batch_with_nulls() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE nullable (id INTEGER PRIMARY KEY, value TEXT)",
        [],
    )
    .unwrap();

    let mut inserter = TypedBatchInserter::new("nullable", &["value"]);

    inserter.add_row(&[Value::Text("has value".to_string())]);
    inserter.add_row(&[Value::Null]);
    inserter.add_row(&[Value::Text("another".to_string())]);

    let count = inserter.execute(&mut conn).unwrap();
    assert_eq!(count, 3);

    let null_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nullable WHERE value IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(null_count, 1);
}

#[test]
fn test_typed_batch_inserter_rejects_row_shape_mismatch() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE products (name TEXT, price REAL)", [])
        .unwrap();

    let mut inserter = TypedBatchInserter::new("products", &["name", "price"]);
    inserter.add_row(&[Value::Text("Widget".to_string())]);

    let err = inserter.execute(&mut conn).unwrap_err();
    assert!(err.to_string().contains("expected 2"));
    assert_eq!(inserter.len(), 1);
}
