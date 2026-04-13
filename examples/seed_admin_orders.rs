use std::{
    env,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use serde_json::json;
use turso::{Builder, params};
use uuid::Uuid;

const MVCC_JOURNAL_MODE_SQL: &str = "PRAGMA journal_mode = 'mvcc'";
const BEGIN_CONCURRENT_SQL: &str = "BEGIN CONCURRENT";
const COMMIT_SQL: &str = "COMMIT";

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args(env::args().skip(1).collect())?;
    let db = Builder::new_local(&args.db_path.to_string_lossy())
        .build()
        .await
        .with_context(|| {
            format!(
                "failed to open turso database at {}",
                args.db_path.display()
            )
        })?;
    let conn = db.connect().context("failed to open turso connection")?;

    let mut rows = conn
        .query(MVCC_JOURNAL_MODE_SQL, ())
        .await
        .context("failed to switch journal_mode to mvcc")?;
    let row = rows
        .next()
        .await
        .context("failed to fetch mvcc pragma row")?
        .ok_or_else(|| anyhow!("mvcc pragma returned no rows"))?;
    let mode = row.get::<String>(0).map_err(|error| anyhow!(error))?;
    if mode.to_lowercase() != "mvcc" {
        return Err(anyhow!("unexpected journal_mode: {}", mode));
    }

    conn.execute(BEGIN_CONCURRENT_SQL, ())
        .await
        .context("failed to begin concurrent seed transaction")?;

    let mut previous_ids = Vec::new();
    let mut rows = conn
        .query("SELECT id FROM orders WHERE src_host = 'admin-smoke'", ())
        .await
        .context("failed to list previous admin smoke orders")?;
    while let Some(row) = rows
        .next()
        .await
        .context("failed to iterate previous admin smoke orders")?
    {
        previous_ids.push(row.get::<String>(0).map_err(|error| anyhow!(error))?);
    }
    for order_id in &previous_ids {
        conn.execute(
            "DELETE FROM audit_logs WHERE order_id = ?1",
            params![order_id.clone()],
        )
        .await
        .with_context(|| format!("failed to delete audit logs for {}", order_id))?;
    }
    conn.execute("DELETE FROM orders WHERE src_host = 'admin-smoke'", ())
        .await
        .context("failed to delete previous admin smoke orders")?;

    let now = now_timestamp();
    let seeded = SeededOrders {
        cancel: Uuid::new_v4().to_string(),
        retry: Uuid::new_v4().to_string(),
        refund: Uuid::new_v4().to_string(),
    };
    let orders = [
        SeedOrder {
            id: seeded.cancel.clone(),
            user_id: args.user_id,
            amount_cents: 2100,
            pay_amount_cents: 2100,
            status: "PENDING",
            payment_trade_no: None,
            expires_at: now + 600,
            paid_at: None,
            completed_at: None,
            failed_at: None,
            failed_reason: None,
            created_at: now - 3,
            updated_at: now - 3,
            recharge_code: generate_recharge_code("cancel"),
        },
        SeedOrder {
            id: seeded.retry.clone(),
            user_id: args.user_id,
            amount_cents: 3200,
            pay_amount_cents: 3200,
            status: "FAILED",
            payment_trade_no: Some("pi_smoke_retry_local".to_string()),
            expires_at: now - 500,
            paid_at: Some(now - 480),
            completed_at: None,
            failed_at: Some(now - 470),
            failed_reason: Some("mock recharge failure".to_string()),
            created_at: now - 20,
            updated_at: now - 20,
            recharge_code: generate_recharge_code("retry"),
        },
        SeedOrder {
            id: seeded.refund.clone(),
            user_id: args.user_id,
            amount_cents: 4500,
            pay_amount_cents: 4500,
            status: "COMPLETED",
            payment_trade_no: None,
            expires_at: now - 1000,
            paid_at: Some(now - 980),
            completed_at: Some(now - 970),
            failed_at: None,
            failed_reason: None,
            created_at: now - 40,
            updated_at: now - 40,
            recharge_code: generate_recharge_code("refund"),
        },
    ];

    for order in &orders {
        conn.execute(
            "INSERT INTO orders (
                id, user_id, amount_cents, pay_amount_cents, fee_rate_bps, recharge_code,
                status, payment_type, payment_trade_no, expires_at, paid_at, completed_at,
                failed_at, failed_reason, created_at, updated_at, src_host, order_type,
                provider_instance_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                order.id.clone(),
                order.user_id,
                order.amount_cents,
                order.pay_amount_cents,
                0,
                order.recharge_code.clone(),
                order.status,
                "stripe",
                order.payment_trade_no.clone(),
                order.expires_at,
                order.paid_at,
                order.completed_at,
                order.failed_at,
                order.failed_reason.clone(),
                order.created_at,
                order.updated_at,
                "admin-smoke",
                "balance",
                Option::<String>::None,
            ],
        )
        .await
        .with_context(|| format!("failed to insert admin smoke order {}", order.id))?;

        conn.execute(
            "INSERT INTO audit_logs (id, order_id, action, detail, operator, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                order.id.clone(),
                "ORDER_CREATED",
                json!({
                    "seed": "admin-smoke",
                    "status": order.status,
                    "amountCents": order.amount_cents,
                })
                .to_string(),
                "seed:admin-smoke",
                order.created_at,
            ],
        )
        .await
        .with_context(|| format!("failed to insert audit log for {}", order.id))?;
    }

    conn.execute(COMMIT_SQL, ())
        .await
        .context("failed to commit admin smoke seed transaction")?;

    println!(
        "{}",
        json!({
            "seededOrders": {
                "cancel": seeded.cancel,
                "retry": seeded.retry,
                "refund": seeded.refund,
            }
        })
    );

    Ok(())
}

#[derive(Debug)]
struct Args {
    db_path: PathBuf,
    user_id: i64,
}

#[derive(Debug, Clone)]
struct SeededOrders {
    cancel: String,
    retry: String,
    refund: String,
}

#[derive(Debug, Clone)]
struct SeedOrder {
    id: String,
    user_id: i64,
    amount_cents: i64,
    pay_amount_cents: i64,
    status: &'static str,
    payment_trade_no: Option<String>,
    expires_at: i64,
    paid_at: Option<i64>,
    completed_at: Option<i64>,
    failed_at: Option<i64>,
    failed_reason: Option<String>,
    created_at: i64,
    updated_at: i64,
    recharge_code: String,
}

fn parse_args(args: Vec<String>) -> Result<Args> {
    let mut db_path = None;
    let mut user_id = 42_i64;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--db-path" => {
                db_path = iter.next().map(PathBuf::from);
            }
            "--user-id" => {
                user_id = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --user-id"))?
                    .parse::<i64>()
                    .context("invalid --user-id")?;
            }
            other => return Err(anyhow!("unknown argument: {}", other)),
        }
    }

    Ok(Args {
        db_path: db_path.unwrap_or_else(|| PathBuf::from("data/opay-smoke.db")),
        user_id,
    })
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock drifted before unix epoch")
        .as_secs() as i64
}

fn generate_recharge_code(label: &str) -> String {
    format!(
        "SMOKE-{}-{}",
        label.to_uppercase(),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}
