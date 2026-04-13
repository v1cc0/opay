use std::{ops::Deref, path::Path, sync::Arc};

use anyhow::{Context, Result, anyhow};
use turso::{Builder, Connection, Database, Error as TursoError, Row};

const MVCC_JOURNAL_MODE_SQL: &str = "PRAGMA journal_mode = 'mvcc'";
const CURRENT_JOURNAL_MODE_SQL: &str = "PRAGMA journal_mode";
const BEGIN_CONCURRENT_SQL: &str = "BEGIN CONCURRENT";
const COMMIT_SQL: &str = "COMMIT";
const ROLLBACK_SQL: &str = "ROLLBACK";

#[derive(Clone)]
pub struct DatabaseHandle {
    inner: Arc<Database>,
}

pub struct ConcurrentTx {
    conn: Connection,
    finished: bool,
}

impl ConcurrentTx {
    fn new(conn: Connection) -> Self {
        Self {
            conn,
            finished: false,
        }
    }

    pub async fn commit(mut self) -> Result<()> {
        self.conn
            .execute(COMMIT_SQL, ())
            .await
            .context("failed to commit concurrent transaction")?;
        self.finished = true;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn rollback(mut self) -> Result<()> {
        self.conn
            .execute(ROLLBACK_SQL, ())
            .await
            .context("failed to rollback concurrent transaction")?;
        self.finished = true;
        Ok(())
    }
}

impl Deref for ConcurrentTx {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DatabaseHandle {
    pub async fn open_local(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create database directory: {}", parent.display())
            })?;
        }

        let db_path = path.to_string_lossy().to_string();

        let db = Builder::new_local(&db_path)
            .build()
            .await
            .with_context(|| format!("failed to open turso database at {}", path.display()))?;

        let handle = Self {
            inner: Arc::new(db),
        };
        handle.enable_mvcc().await?;

        Ok(handle)
    }

    pub fn connect(&self) -> Result<Connection> {
        self.inner
            .connect()
            .context("failed to open turso connection")
    }

    pub async fn begin_concurrent(&self) -> Result<ConcurrentTx> {
        let conn = self.connect()?;
        conn.execute(BEGIN_CONCURRENT_SQL, ())
            .await
            .context("failed to begin concurrent transaction")?;
        Ok(ConcurrentTx::new(conn))
    }

    pub async fn current_journal_mode(&self) -> Result<String> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(CURRENT_JOURNAL_MODE_SQL, ())
            .await
            .context("failed to read current journal mode")?;
        let row = rows
            .next()
            .await
            .context("failed to fetch current journal mode row")?
            .ok_or_else(|| anyhow!("journal mode query returned no rows"))?;
        parse_string_from_row(&row, 0).context("failed to parse current journal mode")
    }

    async fn enable_mvcc(&self) -> Result<()> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(MVCC_JOURNAL_MODE_SQL, ())
            .await
            .context("failed to switch turso journal_mode to mvcc")?;
        let row = rows
            .next()
            .await
            .context("failed to fetch mvcc pragma result row")?
            .ok_or_else(|| anyhow!("mvcc pragma returned no rows"))?;
        let journal_mode =
            parse_string_from_row(&row, 0).context("failed to parse mvcc pragma result")?;
        if journal_mode.to_lowercase() != "mvcc" {
            return Err(anyhow!(
                "unexpected journal_mode after mvcc switch: {}",
                journal_mode
            ));
        }
        Ok(())
    }

    pub async fn ping(&self) -> Result<()> {
        let conn = self.connect()?;
        let mut rows = conn
            .query("SELECT 1", ())
            .await
            .context("database ping query failed")?;

        let Some(_row) = rows
            .next()
            .await
            .context("database ping row fetch failed")?
        else {
            return Err(anyhow!("database ping returned no rows"));
        };

        Ok(())
    }

    pub async fn applied_migration_count(&self) -> Result<i64> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM schema_migrations")
            .await
            .context("failed to prepare schema migration count query")?;
        let row = stmt
            .query_row(())
            .await
            .context("failed to count schema migrations")?;

        parse_i64_from_row(&row, 0).context("failed to parse schema migration count")
    }

    pub async fn run_migrations(&self) -> Result<()> {
        let conn = self.connect()?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at INTEGER NOT NULL DEFAULT (unixepoch()))",
            (),
        )
        .await
        .context("failed to create schema_migrations table")?;

        for migration in MIGRATIONS {
            let mut stmt = conn
                .prepare("SELECT COUNT(*) FROM schema_migrations WHERE name = ?1")
                .await
                .with_context(|| {
                    format!("failed to prepare migration lookup {}", migration.name)
                })?;

            let exists = stmt
                .query_row([migration.name])
                .await
                .with_context(|| format!("failed to check migration {}", migration.name))?;

            let count = parse_i64_from_row(&exists, 0).with_context(|| {
                format!("failed to parse migration count for {}", migration.name)
            })?;

            if count > 0 {
                continue;
            }

            conn.execute_batch(migration.sql)
                .await
                .with_context(|| format!("failed to apply migration {}", migration.name))?;

            conn.execute(
                "INSERT INTO schema_migrations (name) VALUES (?1)",
                [migration.name],
            )
            .await
            .with_context(|| format!("failed to record migration {}", migration.name))?;
        }

        Ok(())
    }
}

pub fn is_retryable_concurrent_error(error: &TursoError) -> bool {
    matches!(error, TursoError::Busy(_) | TursoError::BusySnapshot(_))
        || matches!(error, TursoError::Error(message) if message.contains("conflict"))
}

struct Migration {
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_system_configs",
        sql: include_str!("../migrations/0001_system_configs.sql"),
    },
    Migration {
        name: "0002_core_schema",
        sql: include_str!("../migrations/0002_core_schema.sql"),
    },
];

fn parse_i64_from_row(row: &Row, index: usize) -> Result<i64> {
    row.get::<i64>(index).map_err(|error| anyhow!(error))
}

fn parse_string_from_row(row: &Row, index: usize) -> Result<String> {
    row.get::<String>(index).map_err(|error| anyhow!(error))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn open_local_switches_journal_mode_to_mvcc() {
        let path = std::env::temp_dir().join(format!("opay-db-mvcc-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();

        let journal_mode = db.current_journal_mode().await.unwrap();
        assert_eq!(journal_mode.to_lowercase(), "mvcc");
    }

    #[tokio::test]
    async fn begin_concurrent_allows_write_then_commit() {
        let path = std::env::temp_dir().join(format!("opay-db-concurrent-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT)",
            (),
        )
        .await
        .unwrap();

        let tx = db.begin_concurrent().await.unwrap();
        tx.execute("INSERT INTO items (name) VALUES (?1)", ["hello"])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let conn = db.connect().unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM items WHERE name = ?1", ["hello"])
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(parse_i64_from_row(&row, 0).unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_commit_conflict_on_same_row_is_retryable() {
        let path = std::env::temp_dir().join(format!("opay-db-conflict-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS counters (id INTEGER PRIMARY KEY, val INTEGER NOT NULL)",
            (),
        )
        .await
        .unwrap();
        conn.execute("INSERT INTO counters (id, val) VALUES (1, 0)", ())
            .await
            .unwrap();

        let tx1 = db.begin_concurrent().await.unwrap();
        let tx2 = db.begin_concurrent().await.unwrap();
        tx1.execute("UPDATE counters SET val = val + 1 WHERE id = 1", ())
            .await
            .unwrap();
        let tx2_update = tx2
            .execute("UPDATE counters SET val = val + 1 WHERE id = 1", ())
            .await;

        tx1.commit().await.unwrap();
        match tx2_update {
            Ok(_) => {
                let err = tx2.commit().await.unwrap_err();
                let turso_err = err
                    .downcast_ref::<TursoError>()
                    .expect("expected turso error for concurrent conflict");
                assert!(is_retryable_concurrent_error(turso_err));
            }
            Err(err) => {
                assert!(is_retryable_concurrent_error(&err));
                let _ = tx2.rollback().await;
            }
        }

        let conn = db.connect().unwrap();
        let mut rows = conn
            .query("SELECT val FROM counters WHERE id = 1", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(parse_i64_from_row(&row, 0).unwrap(), 1);
    }
}
