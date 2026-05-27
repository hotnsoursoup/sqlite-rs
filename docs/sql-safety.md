# SQL safety boundary

`sqlite-rs` stays close to `rusqlite`: callers write SQL, pass parameters, and
receive rows. The crate adds pooling, migrations, queues, and utility helpers;
it is not a SQL firewall or ORM.

## Values: bind parameters

Values should be passed through `rusqlite` parameters, not interpolated into SQL
strings.

```rust
pool.write(|conn| {
    conn.execute("INSERT INTO users (name) VALUES (?1)", [&name])
}).await?;
```

`BatchInserter` and `TypedBatchInserter` also bind row values as query
parameters. They do not interpolate values into the generated `INSERT` SQL.

## Identifiers: quoted by helper APIs

The crate quotes identifiers in the helper APIs that construct SQL from table,
column, index, or savepoint names:

- `schema::safe_add_column`, `schema::safe_drop_column`, and
  `schema::safe_create_index`,
- `Savepoint::new`,
- `BatchInserter`, `TypedBatchInserter`, and `insert_many`.

Batch inserters treat the table name and column list as identifiers. Table names
may be schema-qualified, for example `main.users`, which is emitted as
`"main"."users"`.

## Trusted SQL fragments

Some APIs intentionally accept SQL fragments because SQLite has no parameter
syntax for those positions. These inputs must come from trusted application code,
not end users:

| API field/input | Why it is a trusted fragment |
| --- | --- |
| `Migration::baseline(..., sql)` and `Migration::incremental(..., sql)` | Full migration SQL. |
| `ColumnDef::new(name, column_type)` | Column type/constraint fragment after the quoted column name. |
| `ColumnDef::default(value)` | Default expression after `DEFAULT`. |
| `IndexDef::where_clause(clause)` | Partial-index predicate after `WHERE`. |
| SQL strings passed to `pool.read`, `pool.write`, transactions, observers, or profiling helpers | Caller-authored SQL. |

For user-controlled filters, prefer a fixed SQL template and bind values:

```rust
let status = "open";
pool.read(move |conn| {
    conn.query_row(
        "SELECT COUNT(*) FROM tickets WHERE status = ?1",
        [&status],
        |row| row.get::<_, i64>(0),
    )
}).await?;
```

## Identifier input policy

Identifier helper APIs are intended for application-owned names such as table,
column, and index names. Quoting prevents syntax breaks and identifier injection,
but accepting arbitrary user-chosen identifiers is still usually the wrong
application model. Map user choices to known identifiers instead:

```rust
let order_by = match user_sort_key {
    "created" => "created_at",
    "name" => "display_name",
    _ => "id",
};
```

Then use the selected trusted identifier in your SQL template or helper API.
