# Security Policy

## Supported versions

`sqlite-kit` is currently pre-1.0. Security fixes are applied to the active main
branch and to the latest published pre-1.0 line when practical.

## Reporting a vulnerability

Open a private report through GitHub's security advisory flow for this
repository. If that is unavailable, contact the repository owner directly before
posting exploit details publicly.

Please include:

- affected version or commit,
- minimal reproduction steps,
- expected vs. actual impact,
- whether the issue requires untrusted SQL text, untrusted identifiers, file
  system access, or concurrent access to the same database.

## Scope notes

This library does not make raw SQL safe by itself. User-provided query text,
migration SQL, column type fragments, default expressions, and partial-index
predicates must be treated as trusted application code. See
[`docs/sql-safety.md`](docs/sql-safety.md) for the detailed API boundary.
