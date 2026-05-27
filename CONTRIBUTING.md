# Contributing

## Local validation

Run these before opening a PR:

```bash
cargo fmt --all --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test
cargo test --all-features
cargo test --no-default-features
cargo doc --all-features --no-deps
```

MSRV check:

```bash
cargo +1.77.0 check --lib --all-features
cargo +1.77.0 test --lib --all-features
```

## Compatibility rules

- Keep the stated MSRV in `Cargo.toml`, README, and CI aligned.
- Do not rely on `Cargo.lock` for library MSRV compatibility; dependency bounds
  should resolve cleanly without it.
- Avoid public API changes in patch releases.
- If an API accepts SQL text or SQL fragments, document whether the input is
  trusted SQL, a quoted identifier, or a bound value.
- Prefer small, single-responsibility modules over large catch-all files, but do
  not split cohesive code solely to reduce line count.

## Testing expectations

New behavior should usually have a unit test. Concurrency, queueing, migration,
and pool lifecycle changes should include regression tests that fail against the
old behavior.
