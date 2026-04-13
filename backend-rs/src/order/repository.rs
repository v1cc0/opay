use anyhow::{Context, Result, anyhow};
use turso::{Value, params::Params};
use uuid::Uuid;

use crate::{db::DatabaseHandle, order::codegen::generate_recharge_code};

const ORDER_SELECT_COLUMNS: &str = "id, user_id, user_email, user_name, user_notes, amount_cents, pay_amount_cents, fee_rate_bps, recharge_code, status, payment_type, payment_trade_no, pay_url, qr_code, refund_amount_cents, refund_reason, refund_at, force_refund, refund_requested_at, refund_request_reason, refund_requested_by, expires_at, paid_at, completed_at, failed_at, failed_reason, created_at, updated_at, client_ip, src_host, src_url, order_type, plan_id, subscription_group_id, subscription_days, provider_instance_id";

#[derive(Clone)]
pub struct OrderRepository {
    db: DatabaseHandle,
}

#[derive(Debug, Clone)]
pub struct OrderRecord {
    pub id: String,
    pub user_id: i64,
    pub user_email: Option<String>,
    pub user_name: Option<String>,
    pub user_notes: Option<String>,
    pub amount_cents: i64,
    pub pay_amount_cents: Option<i64>,
    pub fee_rate_bps: Option<i64>,
    pub recharge_code: String,
    pub status: String,
    pub payment_type: String,
    pub payment_trade_no: Option<String>,
    pub pay_url: Option<String>,
    pub qr_code: Option<String>,
    pub refund_amount_cents: Option<i64>,
    pub refund_reason: Option<String>,
    pub refund_at: Option<i64>,
    pub force_refund: bool,
    pub refund_requested_at: Option<i64>,
    pub refund_request_reason: Option<String>,
    pub refund_requested_by: Option<i64>,
    pub order_type: String,
    pub plan_id: Option<String>,
    pub subscription_group_id: Option<i64>,
    pub subscription_days: Option<i64>,
    pub provider_instance_id: Option<String>,
    pub expires_at: i64,
    pub paid_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub failed_at: Option<i64>,
    pub failed_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub client_ip: Option<String>,
    pub src_host: Option<String>,
    pub src_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default)]
pub struct AdminOrderListFilters {
    pub status: Option<String>,
    pub order_type: Option<String>,
    pub user_id: Option<i64>,
    pub created_from: Option<i64>,
    pub created_to: Option<i64>,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct NewPendingOrder {
    pub user_id: i64,
    pub amount_cents: i64,
    pub pay_amount_cents: Option<i64>,
    pub fee_rate_bps: Option<i64>,
    pub status: String,
    pub payment_type: String,
    pub order_type: String,
    pub plan_id: Option<String>,
    pub subscription_group_id: Option<i64>,
    pub subscription_days: Option<i64>,
    pub provider_instance_id: Option<String>,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct MarkPaidInput {
    pub order_id: String,
    pub trade_no: String,
    pub paid_amount_cents: i64,
    pub paid_at: i64,
    pub grace_updated_at_gte: i64,
}

#[derive(Debug, Clone)]
pub struct MarkRefundRequestedInput {
    pub order_id: String,
    pub user_id: i64,
    pub refund_amount_cents: i64,
    pub refund_request_reason: Option<String>,
    pub refund_requested_by: i64,
    pub refund_requested_at: i64,
}

#[derive(Debug, Clone)]
pub struct MarkRefundedInput {
    pub order_id: String,
    pub status: String,
    pub refund_amount_cents: i64,
    pub refund_reason: String,
    pub refund_at: i64,
    pub force_refund: bool,
}

impl OrderRepository {
    pub fn new(db: DatabaseHandle) -> Self {
        Self { db }
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<OrderRecord>> {
        let conn = self.db.connect()?;
        let sql = format!("SELECT {ORDER_SELECT_COLUMNS} FROM orders WHERE id = ?1");
        let mut stmt = conn
            .prepare(&sql)
            .await
            .with_context(|| format!("failed to prepare order lookup {id}"))?;
        let mut rows = stmt
            .query([id])
            .await
            .with_context(|| format!("failed to execute order lookup {id}"))?;

        match rows.next().await.context("failed to fetch order row")? {
            Some(row) => Ok(Some(parse_order_row(row)?)),
            None => Ok(None),
        }
    }

    pub async fn list_by_status(&self, status: &str, limit: i64) -> Result<Vec<OrderRecord>> {
        let sql = format!(
            "SELECT {ORDER_SELECT_COLUMNS} FROM orders WHERE status = ?1 ORDER BY created_at ASC LIMIT ?2"
        );
        self.query_orders(
            &sql,
            Params::Positional(vec![Value::Text(status.to_string()), Value::Integer(limit)]),
        )
        .await
        .with_context(|| format!("failed to list orders by status {status}"))
    }

    pub async fn list_expired_pending(&self, now_ts: i64, limit: i64) -> Result<Vec<OrderRecord>> {
        let sql = format!(
            "SELECT {ORDER_SELECT_COLUMNS} FROM orders WHERE status = 'PENDING' AND expires_at < ?1 ORDER BY expires_at ASC LIMIT ?2"
        );
        self.query_orders(
            &sql,
            Params::Positional(vec![Value::Integer(now_ts), Value::Integer(limit)]),
        )
        .await
        .context("failed to list expired pending orders")
    }

    pub async fn count_pending_by_user(&self, user_id: i64) -> Result<i64> {
        let conn = self.db.connect()?;
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM orders WHERE user_id = ?1 AND status = 'PENDING'")
            .await
            .with_context(|| format!("failed to prepare pending order count for user {user_id}"))?;
        let row = stmt
            .query_row([user_id])
            .await
            .with_context(|| format!("failed to count pending orders for user {user_id}"))?;
        row.get::<i64>(0).map_err(|error| anyhow!(error))
    }

    pub async fn count_by_user_total(&self, user_id: i64) -> Result<i64> {
        let conn = self.db.connect()?;
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM orders WHERE user_id = ?1")
            .await
            .with_context(|| format!("failed to prepare total order count for user {user_id}"))?;
        let row = stmt
            .query_row([user_id])
            .await
            .with_context(|| format!("failed to count total orders for user {user_id}"))?;
        row.get::<i64>(0).map_err(|error| anyhow!(error))
    }

    pub async fn count_statuses_by_user(&self, user_id: i64) -> Result<Vec<StatusCount>> {
        let conn = self.db.connect()?;
        let mut rows = conn
            .query(
                "SELECT status, COUNT(*) FROM orders WHERE user_id = ?1 GROUP BY status",
                [user_id],
            )
            .await
            .with_context(|| format!("failed to count statuses for user {user_id}"))?;

        let mut items = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .context("failed to iterate grouped status rows")?
        {
            items.push(StatusCount {
                status: read_required_string(&row.get_value(0)?)?,
                count: read_required_i64(&row.get_value(1)?)?,
            });
        }
        Ok(items)
    }

    pub async fn list_by_user(
        &self,
        user_id: i64,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<OrderRecord>> {
        let sql = format!(
            "SELECT {ORDER_SELECT_COLUMNS} FROM orders WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3"
        );
        self.query_orders(
            &sql,
            Params::Positional(vec![
                Value::Integer(user_id),
                Value::Integer(limit),
                Value::Integer(offset),
            ]),
        )
        .await
        .with_context(|| format!("failed to list orders for user {user_id}"))
    }

    pub async fn list_for_admin(
        &self,
        filters: &AdminOrderListFilters,
    ) -> Result<Vec<OrderRecord>> {
        let (where_clause, mut params) = build_admin_filter_clause(filters);
        let limit_index = params.len() + 1;
        let offset_index = params.len() + 2;
        let sql = format!(
            "SELECT {ORDER_SELECT_COLUMNS} FROM orders{where_clause} ORDER BY created_at DESC LIMIT ?{limit_index} OFFSET ?{offset_index}"
        );
        params.push(Value::Integer(filters.limit));
        params.push(Value::Integer(filters.offset));

        self.query_orders(&sql, Params::Positional(params))
            .await
            .context("failed to list admin orders")
    }

    pub async fn count_for_admin(&self, filters: &AdminOrderListFilters) -> Result<i64> {
        let conn = self.db.connect()?;
        let (where_clause, params) = build_admin_filter_clause(filters);
        let sql = format!("SELECT COUNT(*) FROM orders{where_clause}");
        let mut stmt = conn
            .prepare(&sql)
            .await
            .context("failed to prepare admin order count query")?;
        let row = stmt
            .query_row(Params::Positional(params))
            .await
            .context("failed to count admin orders")?;
        row.get::<i64>(0).map_err(|error| anyhow!(error))
    }

    pub async fn insert_pending(&self, input: NewPendingOrder) -> Result<OrderRecord> {
        let id = Uuid::new_v4().to_string();
        let recharge_code = generate_recharge_code(&id);
        let conn = self.db.connect()?;
        let params = Params::Positional(vec![
            Value::Text(id.clone()),
            Value::Integer(input.user_id),
            Value::Integer(input.amount_cents),
            input
                .pay_amount_cents
                .map(Value::Integer)
                .unwrap_or(Value::Null),
            input
                .fee_rate_bps
                .map(Value::Integer)
                .unwrap_or(Value::Null),
            Value::Text(recharge_code),
            Value::Text(input.status),
            Value::Text(input.payment_type),
            Value::Text(input.order_type),
            input.plan_id.map(Value::Text).unwrap_or(Value::Null),
            input
                .subscription_group_id
                .map(Value::Integer)
                .unwrap_or(Value::Null),
            input
                .subscription_days
                .map(Value::Integer)
                .unwrap_or(Value::Null),
            input
                .provider_instance_id
                .map(Value::Text)
                .unwrap_or(Value::Null),
            Value::Integer(input.expires_at),
        ]);

        conn.execute(
            "INSERT INTO orders (id, user_id, amount_cents, pay_amount_cents, fee_rate_bps, recharge_code, status, payment_type, order_type, plan_id, subscription_group_id, subscription_days, provider_instance_id, expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch(), unixepoch())",
            params,
        )
        .await
        .with_context(|| format!("failed to insert pending order {id}"))?;

        self.get_by_id(&id)
            .await?
            .ok_or_else(|| anyhow!("order {id} disappeared after insert"))
    }

    pub async fn mark_status_if_pending(&self, order_id: &str, status: &str) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders SET status = ?1, updated_at = unixepoch() WHERE id = ?2 AND status = 'PENDING'",
                Params::Positional(vec![
                    Value::Text(status.to_string()),
                    Value::Text(order_id.to_string()),
                ]),
            )
            .await
            .with_context(|| format!("failed to update pending order {order_id} to {status}"))?;
        Ok(affected > 0)
    }

    pub async fn mark_paid_if_pending_or_recent_expired(
        &self,
        input: MarkPaidInput,
    ) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'PAID',
                     pay_amount_cents = ?1,
                     payment_trade_no = ?2,
                     paid_at = ?3,
                     failed_at = NULL,
                     failed_reason = NULL,
                     updated_at = unixepoch()
                 WHERE id = ?4
                   AND (
                     status = 'PENDING'
                     OR (status = 'EXPIRED' AND updated_at >= ?5)
                   )",
                Params::Positional(vec![
                    Value::Integer(input.paid_amount_cents),
                    Value::Text(input.trade_no),
                    Value::Integer(input.paid_at),
                    Value::Text(input.order_id),
                    Value::Integer(input.grace_updated_at_gte),
                ]),
            )
            .await
            .context("failed to mark order as paid")?;
        Ok(affected > 0)
    }

    pub async fn mark_recharging_if_paid_or_failed(&self, order_id: &str) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'RECHARGING',
                     updated_at = unixepoch()
                 WHERE id = ?1
                   AND status IN ('PAID', 'FAILED')",
                [order_id],
            )
            .await
            .with_context(|| format!("failed to lock order {order_id} for fulfillment"))?;
        Ok(affected > 0)
    }

    pub async fn reset_to_paid_if_retryable(&self, order_id: &str) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'PAID',
                     failed_at = NULL,
                     failed_reason = NULL,
                     updated_at = unixepoch()
                 WHERE id = ?1
                   AND status IN ('FAILED', 'PAID')
                   AND paid_at IS NOT NULL",
                [order_id],
            )
            .await
            .with_context(|| format!("failed to reset order {order_id} to PAID for retry"))?;
        Ok(affected > 0)
    }

    pub async fn mark_refund_requested(&self, input: MarkRefundRequestedInput) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'REFUND_REQUESTED',
                     refund_requested_at = ?1,
                     refund_request_reason = ?2,
                     refund_requested_by = ?3,
                     refund_amount_cents = ?4,
                     updated_at = unixepoch()
                 WHERE id = ?5
                   AND user_id = ?6
                   AND status = 'COMPLETED'
                   AND order_type = 'balance'",
                Params::Positional(vec![
                    Value::Integer(input.refund_requested_at),
                    input
                        .refund_request_reason
                        .map(Value::Text)
                        .unwrap_or(Value::Null),
                    Value::Integer(input.refund_requested_by),
                    Value::Integer(input.refund_amount_cents),
                    Value::Text(input.order_id),
                    Value::Integer(input.user_id),
                ]),
            )
            .await
            .context("failed to mark refund requested")?;
        Ok(affected > 0)
    }

    pub async fn mark_refunding_if_refundable(&self, order_id: &str) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'REFUNDING',
                     updated_at = unixepoch()
                 WHERE id = ?1
                   AND status IN ('COMPLETED', 'REFUND_REQUESTED', 'REFUND_FAILED')",
                [order_id],
            )
            .await
            .with_context(|| format!("failed to lock order {order_id} for refund"))?;
        Ok(affected > 0)
    }

    pub async fn restore_after_refund_gateway_failure(
        &self,
        order_id: &str,
        restore_status: &str,
    ) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = ?1,
                     failed_at = NULL,
                     failed_reason = NULL,
                     updated_at = unixepoch()
                 WHERE id = ?2
                   AND status = 'REFUNDING'",
                Params::Positional(vec![
                    Value::Text(restore_status.to_string()),
                    Value::Text(order_id.to_string()),
                ]),
            )
            .await
            .with_context(|| {
                format!("failed to restore order {order_id} after refund gateway failure")
            })?;
        Ok(affected > 0)
    }

    pub async fn mark_refund_failed(
        &self,
        order_id: &str,
        failed_reason: &str,
        failed_at: i64,
    ) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'REFUND_FAILED',
                     failed_at = ?1,
                     failed_reason = ?2,
                     updated_at = unixepoch()
                 WHERE id = ?3
                   AND status = 'REFUNDING'",
                Params::Positional(vec![
                    Value::Integer(failed_at),
                    Value::Text(failed_reason.to_string()),
                    Value::Text(order_id.to_string()),
                ]),
            )
            .await
            .with_context(|| format!("failed to mark order {order_id} refund failed"))?;
        Ok(affected > 0)
    }

    pub async fn mark_refunded(&self, input: MarkRefundedInput) -> Result<bool> {
        let conn = self.db.connect()?;
        let order_id = input.order_id.clone();
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = ?1,
                     refund_amount_cents = ?2,
                     refund_reason = ?3,
                     refund_at = ?4,
                     force_refund = ?5,
                     failed_at = NULL,
                     failed_reason = NULL,
                     updated_at = unixepoch()
                 WHERE id = ?6
                   AND status = 'REFUNDING'",
                Params::Positional(vec![
                    Value::Text(input.status),
                    Value::Integer(input.refund_amount_cents),
                    Value::Text(input.refund_reason),
                    Value::Integer(input.refund_at),
                    Value::Integer(if input.force_refund { 1 } else { 0 }),
                    Value::Text(order_id.clone()),
                ]),
            )
            .await
            .with_context(|| format!("failed to mark order {} refunded", order_id))?;
        Ok(affected > 0)
    }

    pub async fn mark_completed_after_fulfillment(
        &self,
        order_id: &str,
        completed_at: i64,
    ) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'COMPLETED',
                     completed_at = ?1,
                     failed_at = NULL,
                     failed_reason = NULL,
                     updated_at = unixepoch()
                 WHERE id = ?2
                   AND status IN ('RECHARGING', 'PAID', 'FAILED')",
                Params::Positional(vec![
                    Value::Integer(completed_at),
                    Value::Text(order_id.to_string()),
                ]),
            )
            .await
            .with_context(|| format!("failed to mark order {order_id} completed"))?;
        Ok(affected > 0)
    }

    pub async fn mark_failed_if_recharging(
        &self,
        order_id: &str,
        failed_reason: &str,
        failed_at: i64,
    ) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET status = 'FAILED',
                     failed_at = ?1,
                     failed_reason = ?2,
                     updated_at = unixepoch()
                 WHERE id = ?3
                   AND status = 'RECHARGING'",
                Params::Positional(vec![
                    Value::Integer(failed_at),
                    Value::Text(failed_reason.to_string()),
                    Value::Text(order_id.to_string()),
                ]),
            )
            .await
            .with_context(|| format!("failed to mark order {order_id} failed"))?;
        Ok(affected > 0)
    }

    pub async fn set_payment_details(
        &self,
        order_id: &str,
        trade_no: &str,
        pay_url: Option<&str>,
        qr_code: Option<&str>,
    ) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute(
                "UPDATE orders
                 SET payment_trade_no = ?1,
                     pay_url = ?2,
                     qr_code = ?3,
                     updated_at = unixepoch()
                 WHERE id = ?4",
                Params::Positional(vec![
                    Value::Text(trade_no.to_string()),
                    pay_url
                        .map(|value| Value::Text(value.to_string()))
                        .unwrap_or(Value::Null),
                    qr_code
                        .map(|value| Value::Text(value.to_string()))
                        .unwrap_or(Value::Null),
                    Value::Text(order_id.to_string()),
                ]),
            )
            .await
            .with_context(|| format!("failed to set payment details for order {order_id}"))?;
        Ok(affected > 0)
    }

    pub async fn delete_by_id(&self, order_id: &str) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute("DELETE FROM orders WHERE id = ?1", [order_id])
            .await
            .with_context(|| format!("failed to delete order {order_id}"))?;
        Ok(affected > 0)
    }

    async fn query_orders(&self, sql: &str, params: Params) -> Result<Vec<OrderRecord>> {
        let conn = self.db.connect()?;
        let mut rows = conn
            .query(sql, params)
            .await
            .with_context(|| format!("failed to query orders with sql: {sql}"))?;

        let mut items = Vec::new();
        while let Some(row) = rows.next().await.context("failed to iterate order rows")? {
            items.push(parse_order_row(row)?);
        }
        Ok(items)
    }
}

fn parse_order_row(row: turso::Row) -> Result<OrderRecord> {
    Ok(OrderRecord {
        id: read_required_string(&row.get_value(0)?)?,
        user_id: read_required_i64(&row.get_value(1)?)?,
        user_email: read_optional_string(&row.get_value(2)?)?,
        user_name: read_optional_string(&row.get_value(3)?)?,
        user_notes: read_optional_string(&row.get_value(4)?)?,
        amount_cents: read_required_i64(&row.get_value(5)?)?,
        pay_amount_cents: read_optional_i64(&row.get_value(6)?)?,
        fee_rate_bps: read_optional_i64(&row.get_value(7)?)?,
        recharge_code: read_required_string(&row.get_value(8)?)?,
        status: read_required_string(&row.get_value(9)?)?,
        payment_type: read_required_string(&row.get_value(10)?)?,
        payment_trade_no: read_optional_string(&row.get_value(11)?)?,
        pay_url: read_optional_string(&row.get_value(12)?)?,
        qr_code: read_optional_string(&row.get_value(13)?)?,
        refund_amount_cents: read_optional_i64(&row.get_value(14)?)?,
        refund_reason: read_optional_string(&row.get_value(15)?)?,
        refund_at: read_optional_i64(&row.get_value(16)?)?,
        force_refund: read_required_bool(&row.get_value(17)?)?,
        refund_requested_at: read_optional_i64(&row.get_value(18)?)?,
        refund_request_reason: read_optional_string(&row.get_value(19)?)?,
        refund_requested_by: read_optional_i64(&row.get_value(20)?)?,
        expires_at: read_required_i64(&row.get_value(21)?)?,
        paid_at: read_optional_i64(&row.get_value(22)?)?,
        completed_at: read_optional_i64(&row.get_value(23)?)?,
        failed_at: read_optional_i64(&row.get_value(24)?)?,
        failed_reason: read_optional_string(&row.get_value(25)?)?,
        created_at: read_required_i64(&row.get_value(26)?)?,
        updated_at: read_required_i64(&row.get_value(27)?)?,
        client_ip: read_optional_string(&row.get_value(28)?)?,
        src_host: read_optional_string(&row.get_value(29)?)?,
        src_url: read_optional_string(&row.get_value(30)?)?,
        order_type: read_required_string(&row.get_value(31)?)?,
        plan_id: read_optional_string(&row.get_value(32)?)?,
        subscription_group_id: read_optional_i64(&row.get_value(33)?)?,
        subscription_days: read_optional_i64(&row.get_value(34)?)?,
        provider_instance_id: read_optional_string(&row.get_value(35)?)?,
    })
}

fn build_admin_filter_clause(filters: &AdminOrderListFilters) -> (String, Vec<Value>) {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    let mut next_index = 1usize;

    if let Some(status) = filters.status.as_deref() {
        conditions.push(format!("status = ?{next_index}"));
        params.push(Value::Text(status.to_string()));
        next_index += 1;
    }

    if let Some(order_type) = filters.order_type.as_deref() {
        conditions.push(format!("order_type = ?{next_index}"));
        params.push(Value::Text(order_type.to_string()));
        next_index += 1;
    }

    if let Some(user_id) = filters.user_id {
        conditions.push(format!("user_id = ?{next_index}"));
        params.push(Value::Integer(user_id));
        next_index += 1;
    }

    if let Some(created_from) = filters.created_from {
        conditions.push(format!("created_at >= ?{next_index}"));
        params.push(Value::Integer(created_from));
        next_index += 1;
    }

    if let Some(created_to) = filters.created_to {
        conditions.push(format!("created_at <= ?{next_index}"));
        params.push(Value::Integer(created_to));
    }

    if conditions.is_empty() {
        (String::new(), params)
    } else {
        (format!(" WHERE {}", conditions.join(" AND ")), params)
    }
}

fn read_required_string(value: &Value) -> Result<String> {
    match value {
        Value::Text(value) => Ok(value.clone()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Real(value) => Ok(value.to_string()),
        Value::Null => Err(anyhow!("unexpected NULL value")),
        Value::Blob(_) => Err(anyhow!("unexpected BLOB value")),
    }
}

fn read_optional_string(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        _ => read_required_string(value).map(Some),
    }
}

fn read_required_i64(value: &Value) -> Result<i64> {
    match value {
        Value::Integer(value) => Ok(*value),
        Value::Text(value) => value
            .parse::<i64>()
            .context("failed to parse integer from text"),
        Value::Real(value) => Ok(*value as i64),
        _ => Err(anyhow!("unexpected non-integer value")),
    }
}

fn read_optional_i64(value: &Value) -> Result<Option<i64>> {
    match value {
        Value::Null => Ok(None),
        _ => read_required_i64(value).map(Some),
    }
}

fn read_required_bool(value: &Value) -> Result<bool> {
    match value {
        Value::Integer(value) => Ok(*value != 0),
        Value::Text(value) => Ok(value != "0" && !value.eq_ignore_ascii_case("false")),
        Value::Real(value) => Ok(*value != 0.0),
        Value::Null => Err(anyhow!("unexpected NULL value")),
        Value::Blob(_) => Err(anyhow!("unexpected BLOB value")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DatabaseHandle;

    async fn test_repo() -> OrderRepository {
        let path =
            std::env::temp_dir().join(format!("sub2apipay-test-{}.db", uuid::Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();
        OrderRepository::new(db)
    }

    #[tokio::test]
    async fn insert_and_count_pending_order() {
        let repo = test_repo().await;
        let order = repo
            .insert_pending(NewPendingOrder {
                user_id: 7,
                amount_cents: 100,
                pay_amount_cents: Some(120),
                fee_rate_bps: Some(200),
                status: "PENDING".to_string(),
                payment_type: "alipay".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        assert_eq!(order.user_id, 7);
        assert_eq!(repo.count_pending_by_user(7).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_and_expire_pending_orders() {
        let repo = test_repo().await;
        let order = repo
            .insert_pending(NewPendingOrder {
                user_id: 7,
                amount_cents: 100,
                pay_amount_cents: Some(120),
                fee_rate_bps: Some(200),
                status: "PENDING".to_string(),
                payment_type: "alipay".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let expired = repo.list_expired_pending(2000, 10).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, order.id);

        assert!(
            repo.mark_status_if_pending(&order.id, "EXPIRED")
                .await
                .unwrap()
        );
        assert!(
            !repo
                .mark_status_if_pending(&order.id, "EXPIRED")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn mark_paid_with_grace_window() {
        let repo = test_repo().await;
        let order = repo
            .insert_pending(NewPendingOrder {
                user_id: 9,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        assert!(
            repo.mark_paid_if_pending_or_recent_expired(MarkPaidInput {
                order_id: order.id.clone(),
                trade_no: "pi_123".to_string(),
                paid_amount_cents: 550,
                paid_at: 2000,
                grace_updated_at_gte: 0,
            })
            .await
            .unwrap()
        );

        let saved = repo.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "PAID");
        assert_eq!(saved.payment_trade_no.as_deref(), Some("pi_123"));
        assert_eq!(saved.paid_at, Some(2000));
    }

    #[tokio::test]
    async fn fulfillment_status_transitions_are_atomic() {
        let repo = test_repo().await;
        let order = repo
            .insert_pending(NewPendingOrder {
                user_id: 11,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PAID".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        assert!(
            repo.mark_recharging_if_paid_or_failed(&order.id)
                .await
                .unwrap()
        );
        assert!(
            !repo
                .mark_recharging_if_paid_or_failed(&order.id)
                .await
                .unwrap()
        );

        assert!(
            repo.mark_completed_after_fulfillment(&order.id, 3000)
                .await
                .unwrap()
        );

        let saved = repo.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "COMPLETED");
        assert_eq!(saved.completed_at, Some(3000));
        assert_eq!(saved.failed_reason, None);
    }
}
