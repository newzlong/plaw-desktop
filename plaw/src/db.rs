//! SQLite schema-version migration framework.
//!
//! Wraps `PRAGMA user_version` to give every sqlite-backed store in plaw a
//! consistent way to evolve its schema without silently breaking existing
//! user data. Each consumer (memory/capsules.rs, cron/store.rs, etc.)
//! defines its own `&[Migration]` slice and calls [`migrate`] at
//! connection-open time; the helper applies any pending migrations in
//! order, each in its own transaction, and updates `user_version` so the
//! next launch picks up exactly where this one left off.
//!
//! This module is the antidote to the "11 rusqlite consumers, 0 schema
//! versioning" finding from the 2026-05-23 architecture audit — see
//! [[project-2026-05-23-four-lens-synthesis]] Top-4 item #3.
//!
//! # Why per-store names
//!
//! Every connection has exactly one `user_version` integer, so each
//! sqlite *file* is one independent version axis. Different stores
//! living in different files use independent migration slices — they
//! cannot share a version number. The `name` parameter on [`migrate`]
//! is purely for log clarity, not version partitioning.
//!
//! # Example
//!
//! ```no_run
//! use rusqlite::Connection;
//! use plaw::db::{migrate, Migration};
//!
//! let conn = Connection::open("./capsules.db")?;
//! let migrations = &[
//!     Migration {
//!         version: 1,
//!         description: "initial schema",
//!         sql: "CREATE TABLE capsules (id TEXT PRIMARY KEY, body TEXT NOT NULL);",
//!     },
//!     Migration {
//!         version: 2,
//!         description: "add capsules.created_at",
//!         sql: "ALTER TABLE capsules ADD COLUMN created_at TEXT;",
//!     },
//! ];
//! let final_version = migrate(&conn, "capsules", migrations)?;
//! assert_eq!(final_version, 2);
//! # Ok::<_, anyhow::Error>(())
//! ```

use anyhow::{Context, Result};
use rusqlite::Connection;

/// One forward-only schema migration.
///
/// Migrations are identified by `version` (a strictly increasing `u32`).
/// The framework applies them in version order; an attempt to declare two
/// migrations with the same version returns an error from [`migrate`].
///
/// `sql` may contain multiple statements separated by `;`. The framework
/// executes each migration inside a single `BEGIN TRANSACTION ... COMMIT`
/// so a failing statement rolls the whole migration back, leaving
/// `user_version` unchanged.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Strictly increasing version number. `0` is reserved for "uninitialised"
    /// (the default `PRAGMA user_version` of a fresh database).
    pub version: u32,
    /// Short human-readable label, included in tracing output.
    pub description: &'static str,
    /// SQL to apply when stepping from `version - 1` to `version`. May
    /// contain multiple `;`-separated statements; executed in a transaction.
    pub sql: &'static str,
}

/// Apply all pending migrations to `conn`, returning the resulting
/// `user_version`.
///
/// Reads `PRAGMA user_version` first; applies every migration whose version
/// is **greater** than the current; commits each in its own transaction;
/// sets `user_version` to the highest applied version. Returns the new
/// (or unchanged) version on success.
///
/// # Errors
///
/// - `migrations` contains duplicate versions
/// - `migrations` contains version `0` (reserved)
/// - A migration's SQL fails — that migration's transaction rolls back,
///   `user_version` stays at the last successfully applied version, and
///   the error is returned with `name` + failed version context
///
/// # Idempotency
///
/// Calling `migrate` twice in a row with the same slice is safe — the
/// second call sees current_version == max(versions) and applies nothing.
/// Adding new migrations to the end of the slice is the supported
/// evolution path; reordering or rewriting earlier migrations is not
/// (existing users would skip the changes).
pub fn migrate(conn: &Connection, name: &str, migrations: &[Migration]) -> Result<u32> {
    let current = read_user_version(conn)
        .with_context(|| format!("read PRAGMA user_version for store '{name}'"))?;

    if migrations.is_empty() {
        tracing::debug!(store = name, version = current, "no migrations declared");
        return Ok(current);
    }

    validate_no_duplicates(name, migrations)?;
    validate_no_zero(name, migrations)?;

    let mut sorted: Vec<&Migration> = migrations.iter().collect();
    sorted.sort_by_key(|m| m.version);

    let target = sorted.last().map(|m| m.version).unwrap_or(current);
    if target <= current {
        tracing::debug!(
            store = name,
            current,
            target,
            "schema already at or beyond target version"
        );
        return Ok(current);
    }

    let mut applied = current;
    for migration in sorted {
        if migration.version <= current {
            continue;
        }
        apply_one(conn, name, migration)
            .with_context(|| {
                format!(
                    "apply migration {} ({}) on store '{name}'",
                    migration.version, migration.description
                )
            })?;
        applied = migration.version;
    }

    tracing::info!(
        store = name,
        from = current,
        to = applied,
        "schema migration complete"
    );
    Ok(applied)
}

fn read_user_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?;
    Ok(v.clamp(0, i64::from(u32::MAX)) as u32)
}

fn write_user_version(conn: &Connection, version: u32) -> Result<()> {
    // PRAGMA doesn't accept bound parameters; format into the SQL.
    conn.execute_batch(&format!("PRAGMA user_version = {version};"))?;
    Ok(())
}

fn validate_no_duplicates(name: &str, migrations: &[Migration]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for m in migrations {
        if !seen.insert(m.version) {
            anyhow::bail!(
                "store '{name}' has duplicate migration version {}",
                m.version
            );
        }
    }
    Ok(())
}

fn validate_no_zero(name: &str, migrations: &[Migration]) -> Result<()> {
    for m in migrations {
        if m.version == 0 {
            anyhow::bail!(
                "store '{name}' has migration with reserved version 0 ('{}')",
                m.description
            );
        }
    }
    Ok(())
}

fn apply_one(conn: &Connection, name: &str, migration: &Migration) -> Result<()> {
    tracing::info!(
        store = name,
        version = migration.version,
        description = migration.description,
        "applying migration"
    );
    conn.execute_batch("BEGIN;")?;
    let body_result = conn.execute_batch(migration.sql);
    match body_result {
        Ok(()) => {
            if let Err(e) = write_user_version(conn, migration.version) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e.into());
            }
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem() -> Connection {
        Connection::open_in_memory().expect("in-memory sqlite should open")
    }

    #[test]
    fn empty_migrations_returns_current_version() {
        let conn = mem();
        let v = migrate(&conn, "test", &[]).expect("empty slice is no-op");
        assert_eq!(v, 0);
    }

    #[test]
    fn fresh_db_applies_all_in_order() {
        let conn = mem();
        let migrations = &[
            Migration {
                version: 1,
                description: "create t",
                sql: "CREATE TABLE t (id INTEGER PRIMARY KEY);",
            },
            Migration {
                version: 2,
                description: "add t.name",
                sql: "ALTER TABLE t ADD COLUMN name TEXT;",
            },
        ];
        let v = migrate(&conn, "test", migrations).unwrap();
        assert_eq!(v, 2);

        // Verify schema actually applied
        let row: rusqlite::Result<String> = conn
            .query_row("SELECT name FROM pragma_table_info('t') WHERE name='name';", [], |r| r.get(0));
        assert_eq!(row.unwrap(), "name");
    }

    #[test]
    fn second_call_is_idempotent() {
        let conn = mem();
        let migrations = &[Migration {
            version: 1,
            description: "create t",
            sql: "CREATE TABLE t (id INTEGER PRIMARY KEY);",
        }];
        let v1 = migrate(&conn, "test", migrations).unwrap();
        let v2 = migrate(&conn, "test", migrations).unwrap();
        assert_eq!(v1, 1);
        assert_eq!(v2, 1);
    }

    #[test]
    fn adding_new_migration_to_existing_db_applies_only_new() {
        let conn = mem();
        let v1set = &[Migration {
            version: 1,
            description: "v1",
            sql: "CREATE TABLE t (id INTEGER);",
        }];
        migrate(&conn, "test", v1set).unwrap();

        let v2set = &[
            Migration {
                version: 1,
                description: "v1",
                sql: "CREATE TABLE t (id INTEGER);",  // would fail if re-run
            },
            Migration {
                version: 2,
                description: "v2",
                sql: "CREATE TABLE u (id INTEGER);",
            },
        ];
        let v = migrate(&conn, "test", v2set).unwrap();
        assert_eq!(v, 2);
        // Both tables exist
        conn.execute("INSERT INTO t (id) VALUES (1);", []).unwrap();
        conn.execute("INSERT INTO u (id) VALUES (1);", []).unwrap();
    }

    #[test]
    fn migrations_applied_in_version_order_regardless_of_slice_order() {
        let conn = mem();
        let migrations = &[
            Migration {
                version: 2,
                description: "add column",
                sql: "ALTER TABLE t ADD COLUMN name TEXT;",
            },
            Migration {
                version: 1,
                description: "create",
                sql: "CREATE TABLE t (id INTEGER);",
            },
        ];
        let v = migrate(&conn, "test", migrations).unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn duplicate_version_errors() {
        let conn = mem();
        let migrations = &[
            Migration {
                version: 1,
                description: "a",
                sql: "CREATE TABLE a (id INTEGER);",
            },
            Migration {
                version: 1,
                description: "b",
                sql: "CREATE TABLE b (id INTEGER);",
            },
        ];
        let err = migrate(&conn, "test", migrations).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn version_zero_errors() {
        let conn = mem();
        let migrations = &[Migration {
            version: 0,
            description: "reserved",
            sql: "CREATE TABLE t (id INTEGER);",
        }];
        let err = migrate(&conn, "test", migrations).unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn failed_migration_rolls_back_and_leaves_version_unchanged() {
        let conn = mem();
        // First migration creates a table
        let ok_set = &[Migration {
            version: 1,
            description: "create t",
            sql: "CREATE TABLE t (id INTEGER);",
        }];
        migrate(&conn, "test", ok_set).unwrap();

        // Second attempt has v2 with invalid SQL
        let bad_set = &[
            Migration {
                version: 1,
                description: "create t",
                sql: "CREATE TABLE t (id INTEGER);",
            },
            Migration {
                version: 2,
                description: "bad sql",
                sql: "ALTER TABLE nonexistent ADD COLUMN foo TEXT;",
            },
        ];
        let result = migrate(&conn, "test", bad_set);
        assert!(result.is_err());

        // Version should still be 1, NOT 2
        let current: i64 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(current, 1, "failed migration must roll back user_version");
    }

    #[test]
    fn multi_statement_migration_runs_in_one_transaction() {
        let conn = mem();
        let migrations = &[Migration {
            version: 1,
            description: "two tables one migration",
            sql: "CREATE TABLE a (id INTEGER); CREATE TABLE b (id INTEGER);",
        }];
        migrate(&conn, "test", migrations).unwrap();
        conn.execute("INSERT INTO a (id) VALUES (1);", []).unwrap();
        conn.execute("INSERT INTO b (id) VALUES (1);", []).unwrap();
    }
}
