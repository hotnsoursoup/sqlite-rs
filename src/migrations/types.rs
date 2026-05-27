//! Migration type definitions.

/// A database migration.
#[derive(Debug, Clone)]
pub struct Migration {
    /// Version this migration upgrades TO (e.g., "1.2.0").
    pub version: &'static str,

    /// Minimum version required to apply this migration.
    /// For baseline migrations, this is typically "0.0.0".
    pub from_version: &'static str,

    /// Human-readable description of what this migration does.
    pub description: &'static str,

    /// SQL statements to execute for this migration.
    pub sql: &'static str,

    /// Type of migration.
    pub kind: MigrationKind,

    /// Tables created by this migration (for validation).
    pub creates_tables: &'static [&'static str],
}

impl Migration {
    /// Create a baseline migration (for fresh databases).
    ///
    /// # Example
    ///
    /// ```rust
    /// use sqlite_rs::Migration;
    ///
    /// let m = Migration::baseline("1.0.0", r#"
    ///     CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
    ///     CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER);
    /// "#);
    /// ```
    pub const fn baseline(version: &'static str, sql: &'static str) -> Self {
        Self {
            version,
            from_version: "0.0.0",
            description: "Baseline schema",
            sql,
            kind: MigrationKind::Baseline,
            creates_tables: &[],
        }
    }

    /// Create an incremental migration.
    ///
    /// # Example
    ///
    /// ```rust
    /// use sqlite_rs::Migration;
    ///
    /// let m = Migration::incremental("1.1.0", "1.0.0", "ALTER TABLE users ADD COLUMN email TEXT;")
    ///     .with_description("Add email column to users");
    /// ```
    pub const fn incremental(
        version: &'static str,
        from_version: &'static str,
        sql: &'static str,
    ) -> Self {
        Self {
            version,
            from_version,
            description: "",
            sql,
            kind: MigrationKind::Incremental,
            creates_tables: &[],
        }
    }

    /// Add a description to the migration.
    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    /// Specify tables created by this migration (for validation).
    pub const fn creates(mut self, tables: &'static [&'static str]) -> Self {
        self.creates_tables = tables;
        self
    }
}

/// Type of migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationKind {
    /// Baseline migration for fresh databases.
    Baseline,
    /// Incremental migration applied to existing databases.
    Incremental,
}
