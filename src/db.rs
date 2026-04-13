use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, anyhow};
use turso::{Builder, Connection, Database, Row};

#[derive(Clone)]
pub struct DatabaseHandle {
    inner: Arc<Database>,
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

        Ok(Self {
            inner: Arc::new(db),
        })
    }

    pub fn connect(&self) -> Result<Connection> {
        self.inner
            .connect()
            .context("failed to open turso connection")
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
