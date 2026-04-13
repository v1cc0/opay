use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::get,
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use turso::{Value, params::Params};

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    error::{AppError, AppResult},
};

const PAID_STATUSES_SQL: &str =
    "'PAID','RECHARGING','COMPLETED','REFUNDING','REFUNDED','REFUND_FAILED'";
const BIZ_OFFSET_SECONDS: i64 = 8 * 60 * 60;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/dashboard", get(get_dashboard))
}

#[derive(Debug, Deserialize)]
struct DashboardQuery {
    token: Option<String>,
    lang: Option<String>,
    days: Option<i64>,
}

#[derive(Debug, Serialize)]
struct DashboardResponse {
    summary: DashboardSummary,
    #[serde(rename = "dailySeries")]
    daily_series: Vec<DailySeriesItem>,
    leaderboard: Vec<LeaderboardItem>,
    #[serde(rename = "paymentMethods")]
    payment_methods: Vec<PaymentMethodItem>,
    meta: DashboardMeta,
}

#[derive(Debug, Serialize)]
struct DashboardSummary {
    today: SummaryBucket,
    total: SummaryBucket,
    #[serde(rename = "subscriptionToday")]
    subscription_today: SummaryBucket,
    #[serde(rename = "subscriptionTotal")]
    subscription_total: SummaryBucket,
    #[serde(rename = "successRate")]
    success_rate: f64,
    #[serde(rename = "avgAmount")]
    avg_amount: f64,
}

#[derive(Debug, Serialize)]
struct SummaryBucket {
    amount: f64,
    #[serde(rename = "orderCount")]
    order_count: i64,
    #[serde(rename = "paidCount")]
    paid_count: i64,
}

#[derive(Debug, Serialize)]
struct DailySeriesItem {
    date: String,
    amount: f64,
    count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaderboardItem {
    user_id: i64,
    user_name: Option<String>,
    user_email: Option<String>,
    total_amount: f64,
    order_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentMethodItem {
    payment_type: String,
    amount: f64,
    count: i64,
    percentage: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardMeta {
    days: i64,
    generated_at: String,
}

#[derive(Debug)]
struct AggregateRow {
    amount_cents: i64,
    count: i64,
}

async fn get_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DashboardQuery>,
) -> AppResult<Json<DashboardResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let days = query.days.unwrap_or(30).clamp(1, 365);
    let now_ts = now_timestamp();
    let response = build_dashboard_response(&state, days, now_ts).await?;
    Ok(Json(response))
}

async fn build_dashboard_response(
    state: &AppState,
    days: i64,
    now_ts: i64,
) -> AppResult<DashboardResponse> {
    let conn = state.db.connect().map_err(AppError::internal)?;
    let today_start = get_biz_day_start_utc_timestamp(now_ts);
    let start_ts = today_start - days * 86_400;

    let today_paid = query_aggregate(
        &conn,
        &format!(
            "SELECT COALESCE(SUM(amount_cents), 0), COUNT(*) FROM orders WHERE status IN ({PAID_STATUSES_SQL}) AND paid_at >= ?1"
        ),
        Params::Positional(vec![Value::Integer(today_start)]),
    )
    .await?;
    let total_paid = query_aggregate(
        &conn,
        &format!("SELECT COALESCE(SUM(amount_cents), 0), COUNT(*) FROM orders WHERE status IN ({PAID_STATUSES_SQL})"),
        Params::Positional(vec![]),
    )
    .await?;
    let today_orders = query_count(
        &conn,
        "SELECT COUNT(*) FROM orders WHERE created_at >= ?1",
        Params::Positional(vec![Value::Integer(today_start)]),
    )
    .await?;
    let total_orders = query_count(
        &conn,
        "SELECT COUNT(*) FROM orders",
        Params::Positional(vec![]),
    )
    .await?;

    let sub_today_paid = query_aggregate(
        &conn,
        &format!(
            "SELECT COALESCE(SUM(amount_cents), 0), COUNT(*) FROM orders WHERE status IN ({PAID_STATUSES_SQL}) AND paid_at >= ?1 AND order_type = 'subscription'"
        ),
        Params::Positional(vec![Value::Integer(today_start)]),
    )
    .await?;
    let sub_total_paid = query_aggregate(
        &conn,
        &format!(
            "SELECT COALESCE(SUM(amount_cents), 0), COUNT(*) FROM orders WHERE status IN ({PAID_STATUSES_SQL}) AND order_type = 'subscription'"
        ),
        Params::Positional(vec![]),
    )
    .await?;
    let sub_today_orders = query_count(
        &conn,
        "SELECT COUNT(*) FROM orders WHERE created_at >= ?1 AND order_type = 'subscription'",
        Params::Positional(vec![Value::Integer(today_start)]),
    )
    .await?;
    let sub_total_orders = query_count(
        &conn,
        "SELECT COUNT(*) FROM orders WHERE order_type = 'subscription'",
        Params::Positional(vec![]),
    )
    .await?;

    let mut daily_rows = query_daily_series(&conn, start_ts).await?;
    let mut daily_map = std::collections::HashMap::new();
    for item in daily_rows.drain(..) {
        daily_map.insert(item.date.clone(), item);
    }
    let mut daily_series = Vec::new();
    let mut cursor = start_ts;
    while cursor <= now_ts {
        let date = biz_date_str(cursor);
        let item = daily_map.remove(&date).unwrap_or(DailySeriesItem {
            date,
            amount: 0.0,
            count: 0,
        });
        daily_series.push(item);
        cursor += 86_400;
    }

    let leaderboard = query_leaderboard(&conn, start_ts).await?;
    let payment_methods = query_payment_methods(&conn, start_ts).await?;
    let payment_total = payment_methods.iter().map(|item| item.amount).sum::<f64>();
    let payment_methods = payment_methods
        .into_iter()
        .map(|mut item| {
            item.percentage = if payment_total > 0.0 {
                ((item.amount / payment_total) * 1000.0).round() / 10.0
            } else {
                0.0
            };
            item
        })
        .collect::<Vec<_>>();

    let success_rate = if total_orders > 0 {
        ((total_paid.count as f64 / total_orders as f64) * 1000.0).round() / 10.0
    } else {
        0.0
    };
    let avg_amount = if total_paid.count > 0 {
        (cents_to_amount(total_paid.amount_cents) / total_paid.count as f64 * 100.0).round() / 100.0
    } else {
        0.0
    };

    Ok(DashboardResponse {
        summary: DashboardSummary {
            today: SummaryBucket {
                amount: cents_to_amount(today_paid.amount_cents),
                order_count: today_orders,
                paid_count: today_paid.count,
            },
            total: SummaryBucket {
                amount: cents_to_amount(total_paid.amount_cents),
                order_count: total_orders,
                paid_count: total_paid.count,
            },
            subscription_today: SummaryBucket {
                amount: cents_to_amount(sub_today_paid.amount_cents),
                order_count: sub_today_orders,
                paid_count: sub_today_paid.count,
            },
            subscription_total: SummaryBucket {
                amount: cents_to_amount(sub_total_paid.amount_cents),
                order_count: sub_total_orders,
                paid_count: sub_total_paid.count,
            },
            success_rate,
            avg_amount,
        },
        daily_series,
        leaderboard,
        payment_methods,
        meta: DashboardMeta {
            days,
            generated_at: timestamp_to_rfc3339(now_ts),
        },
    })
}

async fn query_aggregate(
    conn: &turso::Connection,
    sql: &str,
    params: Params,
) -> AppResult<AggregateRow> {
    let mut stmt = conn.prepare(sql).await.map_err(AppError::internal)?;
    let row = stmt.query_row(params).await.map_err(AppError::internal)?;
    Ok(AggregateRow {
        amount_cents: read_required_i64(&row.get_value(0).map_err(AppError::internal)?)
            .map_err(AppError::internal)?,
        count: read_required_i64(&row.get_value(1).map_err(AppError::internal)?)
            .map_err(AppError::internal)?,
    })
}

async fn query_count(conn: &turso::Connection, sql: &str, params: Params) -> AppResult<i64> {
    let mut stmt = conn.prepare(sql).await.map_err(AppError::internal)?;
    let row = stmt.query_row(params).await.map_err(AppError::internal)?;
    read_required_i64(&row.get_value(0).map_err(AppError::internal)?).map_err(AppError::internal)
}

async fn query_daily_series(
    conn: &turso::Connection,
    start_ts: i64,
) -> AppResult<Vec<DailySeriesItem>> {
    let mut rows = conn
        .query(
            &format!(
                "SELECT strftime('%Y-%m-%d', paid_at + {BIZ_OFFSET_SECONDS}, 'unixepoch') AS biz_date,
                        COALESCE(SUM(amount_cents), 0) AS amount_cents,
                        COUNT(*) AS order_count
                 FROM orders
                 WHERE status IN ({PAID_STATUSES_SQL})
                   AND paid_at >= ?1
                 GROUP BY biz_date
                 ORDER BY biz_date"
            ),
            Params::Positional(vec![Value::Integer(start_ts)]),
        )
        .await
        .map_err(AppError::internal)?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(AppError::internal)? {
        let date = read_required_string(&row.get_value(0).map_err(AppError::internal)?)
            .map_err(AppError::internal)?;
        let amount_cents = read_required_i64(&row.get_value(1).map_err(AppError::internal)?)
            .map_err(AppError::internal)?;
        let count = read_required_i64(&row.get_value(2).map_err(AppError::internal)?)
            .map_err(AppError::internal)?;
        items.push(DailySeriesItem {
            date,
            amount: cents_to_amount(amount_cents),
            count,
        });
    }
    Ok(items)
}

async fn query_leaderboard(
    conn: &turso::Connection,
    start_ts: i64,
) -> AppResult<Vec<LeaderboardItem>> {
    let mut rows = conn
        .query(
            &format!(
                "SELECT user_id,
                        MAX(user_name) AS user_name,
                        MAX(user_email) AS user_email,
                        COALESCE(SUM(amount_cents), 0) AS total_amount_cents,
                        COUNT(*) AS order_count
                 FROM orders
                 WHERE status IN ({PAID_STATUSES_SQL})
                   AND paid_at >= ?1
                 GROUP BY user_id
                 ORDER BY total_amount_cents DESC
                 LIMIT 10"
            ),
            Params::Positional(vec![Value::Integer(start_ts)]),
        )
        .await
        .map_err(AppError::internal)?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(AppError::internal)? {
        items.push(LeaderboardItem {
            user_id: read_required_i64(&row.get_value(0).map_err(AppError::internal)?)
                .map_err(AppError::internal)?,
            user_name: read_optional_string(&row.get_value(1).map_err(AppError::internal)?)
                .map_err(AppError::internal)?,
            user_email: read_optional_string(&row.get_value(2).map_err(AppError::internal)?)
                .map_err(AppError::internal)?,
            total_amount: cents_to_amount(
                read_required_i64(&row.get_value(3).map_err(AppError::internal)?)
                    .map_err(AppError::internal)?,
            ),
            order_count: read_required_i64(&row.get_value(4).map_err(AppError::internal)?)
                .map_err(AppError::internal)?,
        });
    }
    Ok(items)
}

async fn query_payment_methods(
    conn: &turso::Connection,
    start_ts: i64,
) -> AppResult<Vec<PaymentMethodItem>> {
    let mut rows = conn
        .query(
            &format!(
                "SELECT payment_type,
                        COALESCE(SUM(amount_cents), 0) AS total_amount_cents,
                        COUNT(*) AS order_count
                 FROM orders
                 WHERE status IN ({PAID_STATUSES_SQL})
                   AND paid_at >= ?1
                 GROUP BY payment_type
                 ORDER BY total_amount_cents DESC"
            ),
            Params::Positional(vec![Value::Integer(start_ts)]),
        )
        .await
        .map_err(AppError::internal)?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(AppError::internal)? {
        items.push(PaymentMethodItem {
            payment_type: read_required_string(&row.get_value(0).map_err(AppError::internal)?)
                .map_err(AppError::internal)?,
            amount: cents_to_amount(
                read_required_i64(&row.get_value(1).map_err(AppError::internal)?)
                    .map_err(AppError::internal)?,
            ),
            count: read_required_i64(&row.get_value(2).map_err(AppError::internal)?)
                .map_err(AppError::internal)?,
            percentage: 0.0,
        });
    }
    Ok(items)
}

fn get_biz_day_start_utc_timestamp(now_ts: i64) -> i64 {
    let biz_day = (now_ts + BIZ_OFFSET_SECONDS) / 86_400;
    biz_day * 86_400 - BIZ_OFFSET_SECONDS
}

fn biz_date_str(timestamp: i64) -> String {
    Utc.timestamp_opt(timestamp + BIZ_OFFSET_SECONDS, 0)
        .single()
        .map(|item| item.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn timestamp_to_rfc3339(value: i64) -> String {
    Utc.timestamp_opt(value, 0)
        .single()
        .map(|item| item.to_rfc3339())
        .unwrap_or_else(|| value.to_string())
}

fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock drifted before unix epoch")
        .as_secs() as i64
}

fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
}

fn read_required_string(value: &Value) -> anyhow::Result<String> {
    match value {
        Value::Text(value) => Ok(value.clone()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Real(value) => Ok(value.to_string()),
        Value::Null => Err(anyhow::anyhow!("unexpected NULL value")),
        Value::Blob(_) => Err(anyhow::anyhow!("unexpected BLOB value")),
    }
}

fn read_optional_string(value: &Value) -> anyhow::Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        _ => read_required_string(value).map(Some),
    }
}

fn read_required_i64(value: &Value) -> anyhow::Result<i64> {
    match value {
        Value::Integer(value) => Ok(*value),
        Value::Text(value) => value
            .parse::<i64>()
            .map_err(|error| anyhow::anyhow!("failed to parse integer from text: {error}")),
        Value::Real(value) => Ok(*value as i64),
        _ => Err(anyhow::anyhow!("unexpected non-integer value")),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use turso::{Value, params::Params};

    use super::*;
    use crate::{
        config::AppConfig,
        db::DatabaseHandle,
        order::{audit::AuditLogRepository, repository::OrderRepository, service::OrderService},
        subscription_plan::SubscriptionPlanRepository,
        system_config::SystemConfigService,
    };

    async fn test_state() -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "opay-admin-dashboard-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&db_path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            db_path,
            payment_providers: Vec::new(),
            admin_token: Some("test-admin-token".to_string()),
            system_config_cache_ttl_secs: 1,
            sub2api_base_url: None,
            sub2api_timeout_secs: 2,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(1));
        AppState {
            config: Arc::clone(&config),
            db: db.clone(),
            system_config: system_config.clone(),
            sub2api: None,
            order_service: OrderService::new(
                Arc::clone(&config),
                OrderRepository::new(db.clone()),
                AuditLogRepository::new(db.clone()),
                SubscriptionPlanRepository::new(db.clone()),
                system_config,
                None,
            ),
        }
    }

    async fn insert_order(
        state: &AppState,
        id: &str,
        user_id: i64,
        user_name: &str,
        user_email: &str,
        amount_cents: i64,
        status: &str,
        payment_type: &str,
        order_type: &str,
        created_at: i64,
        paid_at: Option<i64>,
    ) {
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO orders (id, user_id, user_name, user_email, amount_cents, recharge_code, status, payment_type, order_type, expires_at, paid_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            Params::Positional(vec![
                Value::Text(id.to_string()),
                Value::Integer(user_id),
                Value::Text(user_name.to_string()),
                Value::Text(user_email.to_string()),
                Value::Integer(amount_cents),
                Value::Text(format!("code_{id}")),
                Value::Text(status.to_string()),
                Value::Text(payment_type.to_string()),
                Value::Text(order_type.to_string()),
                Value::Integer(created_at + 600),
                paid_at.map(Value::Integer).unwrap_or(Value::Null),
                Value::Integer(created_at),
                Value::Integer(created_at),
            ]),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dashboard_aggregates_summary_series_and_leaderboard() {
        let state = test_state().await;
        let now_ts = 1_775_404_800; // 2026-04-13T00:00:00Z
        let today_start = get_biz_day_start_utc_timestamp(now_ts);

        insert_order(
            &state,
            "o1",
            1,
            "alice",
            "alice@example.com",
            1000,
            "COMPLETED",
            "stripe",
            "balance",
            today_start + 100,
            Some(today_start + 200),
        )
        .await;
        insert_order(
            &state,
            "o2",
            2,
            "bob",
            "bob@example.com",
            2000,
            "REFUNDED",
            "alipay",
            "subscription",
            today_start - 86_400 + 100,
            Some(today_start - 86_400 + 200),
        )
        .await;
        insert_order(
            &state,
            "o3",
            3,
            "charlie",
            "charlie@example.com",
            500,
            "PENDING",
            "wxpay",
            "balance",
            today_start + 300,
            None,
        )
        .await;

        let response = build_dashboard_response(&state, 7, now_ts).await.unwrap();

        assert_eq!(response.summary.today.amount, 10.0);
        assert_eq!(response.summary.today.paid_count, 1);
        assert_eq!(response.summary.today.order_count, 2);
        assert_eq!(response.summary.total.amount, 30.0);
        assert_eq!(response.summary.total.paid_count, 2);
        assert_eq!(response.summary.total.order_count, 3);
        assert_eq!(response.summary.subscription_total.amount, 20.0);
        assert_eq!(response.payment_methods.len(), 2);
        assert_eq!(response.leaderboard[0].user_id, 2);
        assert!(response.daily_series.iter().any(|item| item.amount > 0.0));
    }
}
