#[allow(dead_code)]
#[path = "../src/db.rs"]
mod db;

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use serde_json::json;
use tokio::sync::Barrier;
use turso::Error as TursoError;
use uuid::Uuid;

use db::{DatabaseHandle, is_retryable_concurrent_error};

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::temp_dir().join(format!("opay-concurrent-smoke-{}.db", Uuid::new_v4()));
    let db = DatabaseHandle::open_local(&path).await?;

    let conflict = run_same_row_conflict(&db).await?;
    let competition = run_multi_writer_competition(&db).await?;

    println!(
        "{}",
        json!({
            "dbPath": path,
            "journalMode": db.current_journal_mode().await?,
            "sameRowConflict": conflict,
            "multiWriterCompetition": competition,
        })
    );

    Ok(())
}

async fn run_same_row_conflict(db: &DatabaseHandle) -> Result<serde_json::Value> {
    let conn = db.connect()?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS counters (id INTEGER PRIMARY KEY, val INTEGER NOT NULL)",
        (),
    )
    .await
    .context("failed to create counters table")?;
    conn.execute("DELETE FROM counters", ())
        .await
        .context("failed to clear counters table")?;
    conn.execute("INSERT INTO counters (id, val) VALUES (1, 0)", ())
        .await
        .context("failed to seed counters table")?;

    let tx1 = db.begin_concurrent().await?;
    let tx2 = db.begin_concurrent().await?;
    tx1.execute("UPDATE counters SET val = val + 1 WHERE id = 1", ())
        .await
        .context("failed to update counter in tx1")?;
    let tx2_update = tx2
        .execute("UPDATE counters SET val = val + 1 WHERE id = 1", ())
        .await;

    tx1.commit().await.context("failed to commit tx1")?;
    let retryable = match tx2_update {
        Ok(_) => {
            let tx2_err = tx2
                .commit()
                .await
                .expect_err("tx2 should conflict on commit");
            tx2_err
                .downcast_ref::<TursoError>()
                .map(is_retryable_concurrent_error)
                .unwrap_or(false)
        }
        Err(err) => {
            let retryable = is_retryable_concurrent_error(&err);
            if retryable {
                let _ = tx2.rollback().await;
            }
            retryable
        }
    };
    if !retryable {
        return Err(anyhow!("same-row conflict was not retryable"));
    }

    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT val FROM counters WHERE id = 1", ())
        .await
        .context("failed to query final counter value")?;
    let row = rows
        .next()
        .await
        .context("failed to fetch final counter row")?
        .ok_or_else(|| anyhow!("counter row disappeared"))?;
    let final_value = row.get::<i64>(0).map_err(|error| anyhow!(error))?;

    Ok(json!({
        "retryableConflict": retryable,
        "finalValue": final_value,
    }))
}

async fn run_multi_writer_competition(db: &DatabaseHandle) -> Result<serde_json::Value> {
    let conn = db.connect()?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS competition_results (id INTEGER PRIMARY KEY, worker INTEGER NOT NULL, attempts INTEGER NOT NULL)",
        (),
    )
    .await
    .context("failed to create competition_results table")?;
    conn.execute("DELETE FROM competition_results", ())
        .await
        .context("failed to clear competition_results table")?;

    let worker_count: usize = 8;
    let barrier = Arc::new(Barrier::new(worker_count));
    let mut handles = Vec::new();
    for worker in 0..worker_count {
        let db = db.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut attempts = 0_i64;
            let row_id = (worker as i64) + 1;
            loop {
                attempts += 1;
                let tx = db.begin_concurrent().await?;
                match tx
                    .execute(
                        "INSERT INTO competition_results (id, worker, attempts) VALUES (?1, ?2, ?3)",
                        (row_id, row_id, attempts),
                    )
                    .await
                {
                    Ok(_) => {
                        tx.commit().await?;
                        return Ok::<i64, anyhow::Error>(attempts);
                    }
                    Err(err) => {
                        let retryable = is_retryable_concurrent_error(&err);
                        if retryable {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                        return Err(anyhow!(err))
                            .context(format!("worker {worker} failed permanently"));
                    }
                }
            }
        }));
    }

    let mut max_attempts = 0_i64;
    for handle in handles {
        let attempts = handle.await.context("competition worker panicked")??;
        max_attempts = max_attempts.max(attempts);
    }

    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT COUNT(*) FROM competition_results", ())
        .await
        .context("failed to count competition rows")?;
    let row = rows
        .next()
        .await
        .context("failed to fetch competition count row")?
        .ok_or_else(|| anyhow!("competition count row missing"))?;
    let total_rows = row.get::<i64>(0).map_err(|error| anyhow!(error))?;

    Ok(json!({
        "workerCount": worker_count,
        "totalRows": total_rows,
        "maxAttempts": max_attempts,
    }))
}
