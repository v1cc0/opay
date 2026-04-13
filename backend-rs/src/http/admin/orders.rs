use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    error::{AppError, AppResult},
    http::common::{
        cents_to_amount, message, optional_timestamp_to_rfc3339, resolve_locale,
        timestamp_to_rfc3339,
    },
    order::{
        audit::{AuditLogRecord, AuditLogRepository},
        repository::{AdminOrderListFilters, OrderRecord, OrderRepository},
        service::CancelOrderOutcome,
        status::{RechargeStatus, derive_order_state, is_recharge_retryable},
    },
};

const VALID_ORDER_TYPES: &[&str] = &["balance", "subscription"];
const VALID_STATUSES: &[&str] = &[
    "PENDING",
    "PAID",
    "RECHARGING",
    "COMPLETED",
    "REFUND_REQUESTED",
    "REFUNDING",
    "PARTIALLY_REFUNDED",
    "EXPIRED",
    "CANCELLED",
    "FAILED",
    "REFUNDED",
    "REFUND_FAILED",
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/orders", get(list_orders))
        .route("/api/admin/orders/{id}", get(get_order))
        .route("/api/admin/orders/{id}/retry", post(retry_order))
        .route("/api/admin/orders/{id}/cancel", post(cancel_order))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminOrdersQuery {
    token: Option<String>,
    lang: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
    status: Option<String>,
    order_type: Option<String>,
    user_id: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminOrdersResponse {
    orders: Vec<AdminOrderListItem>,
    total: i64,
    page: i64,
    page_size: i64,
    total_pages: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOrderListItem {
    id: String,
    user_id: i64,
    user_name: Option<String>,
    user_email: Option<String>,
    user_notes: Option<String>,
    amount: f64,
    pay_amount: Option<f64>,
    status: String,
    payment_type: String,
    created_at: String,
    paid_at: Option<String>,
    completed_at: Option<String>,
    failed_reason: Option<String>,
    expires_at: String,
    src_host: Option<String>,
    order_type: String,
    plan_id: Option<String>,
    subscription_group_id: Option<i64>,
    subscription_days: Option<i64>,
    refund_amount: Option<f64>,
    refund_at: Option<String>,
    refund_requested_at: Option<String>,
    refund_request_reason: Option<String>,
    refund_requested_by: Option<i64>,
    recharge_retryable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOrderDetailResponse {
    id: String,
    user_id: i64,
    user_name: Option<String>,
    user_email: Option<String>,
    user_notes: Option<String>,
    amount: f64,
    pay_amount: Option<f64>,
    fee_rate: Option<f64>,
    status: String,
    payment_type: String,
    recharge_code: String,
    payment_trade_no: Option<String>,
    refund_amount: Option<f64>,
    refund_reason: Option<String>,
    refund_at: Option<String>,
    force_refund: bool,
    refund_requested_at: Option<String>,
    refund_request_reason: Option<String>,
    refund_requested_by: Option<i64>,
    expires_at: String,
    paid_at: Option<String>,
    completed_at: Option<String>,
    failed_at: Option<String>,
    failed_reason: Option<String>,
    created_at: String,
    updated_at: String,
    client_ip: Option<String>,
    src_host: Option<String>,
    src_url: Option<String>,
    payment_success: bool,
    recharge_success: bool,
    recharge_status: String,
    order_type: String,
    plan_id: Option<String>,
    subscription_group_id: Option<i64>,
    subscription_days: Option<i64>,
    audit_logs: Vec<AuditLogView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditLogView {
    id: String,
    action: String,
    detail: Option<String>,
    operator: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct ActionSuccessResponse {
    success: bool,
}

#[derive(Debug, Serialize)]
struct CancelOrderResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

async fn list_orders(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminOrdersQuery>,
) -> AppResult<Json<AdminOrdersResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let filters = AdminOrderListFilters {
        status: query
            .status
            .as_deref()
            .map(str::trim)
            .filter(|value| VALID_STATUSES.contains(value))
            .map(str::to_string),
        order_type: query
            .order_type
            .as_deref()
            .map(str::trim)
            .filter(|value| VALID_ORDER_TYPES.contains(value))
            .map(str::to_string),
        user_id: query
            .user_id
            .as_deref()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|value| *value > 0),
        created_from: query
            .date_from
            .as_deref()
            .and_then(|value| parse_query_timestamp(value, false)),
        created_to: query
            .date_to
            .as_deref()
            .and_then(|value| parse_query_timestamp(value, true)),
        offset: (page - 1) * page_size,
        limit: page_size,
    };

    let repo = OrderRepository::new(state.db.clone());
    let orders = repo
        .list_for_admin(&filters)
        .await
        .map_err(AppError::internal)?;
    let total = repo
        .count_for_admin(&filters)
        .await
        .map_err(AppError::internal)?;

    Ok(Json(AdminOrdersResponse {
        orders: orders.into_iter().map(to_admin_order_list_item).collect(),
        total,
        page,
        page_size,
        total_pages: total_pages(total, page_size),
    }))
}

async fn get_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminOrdersQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<AdminOrderDetailResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let order_repo = OrderRepository::new(state.db.clone());
    let audit_repo = AuditLogRepository::new(state.db.clone());
    let order = order_repo
        .get_by_id(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found(message(locale, "订单不存在", "Order not found")))?;
    let audit_logs = audit_repo
        .list_by_order(&id)
        .await
        .map_err(AppError::internal)?;

    Ok(Json(to_admin_order_detail(order, audit_logs)))
}

async fn retry_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminOrdersQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<ActionSuccessResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    state
        .order_service
        .retry_recharge(&id)
        .await
        .map_err(|error| map_retry_error(error, locale))?;

    Ok(Json(ActionSuccessResponse { success: true }))
}

async fn cancel_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminOrdersQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<CancelOrderResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    match state
        .order_service
        .admin_cancel_order(&id)
        .await
        .map_err(|error| map_cancel_error(error, locale))?
    {
        CancelOrderOutcome::Cancelled => Ok(Json(CancelOrderResponse {
            success: true,
            status: None,
            message: None,
        })),
        CancelOrderOutcome::AlreadyPaid => Ok(Json(CancelOrderResponse {
            success: true,
            status: Some("PAID".to_string()),
            message: Some(
                message(locale, "订单已支付完成", "Order has already been paid").to_string(),
            ),
        })),
    }
}

fn to_admin_order_list_item(order: OrderRecord) -> AdminOrderListItem {
    let recharge_retryable = is_recharge_retryable(&order);

    AdminOrderListItem {
        id: order.id,
        user_id: order.user_id,
        user_name: order.user_name,
        user_email: order.user_email,
        user_notes: order.user_notes,
        amount: cents_to_amount(order.amount_cents),
        pay_amount: order.pay_amount_cents.map(cents_to_amount),
        status: order.status.clone(),
        payment_type: order.payment_type,
        created_at: timestamp_to_rfc3339(order.created_at),
        paid_at: optional_timestamp_to_rfc3339(order.paid_at),
        completed_at: optional_timestamp_to_rfc3339(order.completed_at),
        failed_reason: order.failed_reason,
        expires_at: timestamp_to_rfc3339(order.expires_at),
        src_host: order.src_host,
        order_type: order.order_type,
        plan_id: order.plan_id,
        subscription_group_id: order.subscription_group_id,
        subscription_days: order.subscription_days,
        refund_amount: order.refund_amount_cents.map(cents_to_amount),
        refund_at: optional_timestamp_to_rfc3339(order.refund_at),
        refund_requested_at: optional_timestamp_to_rfc3339(order.refund_requested_at),
        refund_request_reason: order.refund_request_reason,
        refund_requested_by: order.refund_requested_by,
        recharge_retryable,
    }
}

fn to_admin_order_detail(
    order: OrderRecord,
    audit_logs: Vec<AuditLogRecord>,
) -> AdminOrderDetailResponse {
    let derived = derive_order_state(&order);

    AdminOrderDetailResponse {
        id: order.id,
        user_id: order.user_id,
        user_name: order.user_name,
        user_email: order.user_email,
        user_notes: order.user_notes,
        amount: cents_to_amount(order.amount_cents),
        pay_amount: order.pay_amount_cents.map(cents_to_amount),
        fee_rate: order.fee_rate_bps.map(|value| (value as f64) / 10_000.0),
        status: order.status,
        payment_type: order.payment_type,
        recharge_code: order.recharge_code,
        payment_trade_no: order.payment_trade_no,
        refund_amount: order.refund_amount_cents.map(cents_to_amount),
        refund_reason: order.refund_reason,
        refund_at: optional_timestamp_to_rfc3339(order.refund_at),
        force_refund: order.force_refund,
        refund_requested_at: optional_timestamp_to_rfc3339(order.refund_requested_at),
        refund_request_reason: order.refund_request_reason,
        refund_requested_by: order.refund_requested_by,
        expires_at: timestamp_to_rfc3339(order.expires_at),
        paid_at: optional_timestamp_to_rfc3339(order.paid_at),
        completed_at: optional_timestamp_to_rfc3339(order.completed_at),
        failed_at: optional_timestamp_to_rfc3339(order.failed_at),
        failed_reason: order.failed_reason,
        created_at: timestamp_to_rfc3339(order.created_at),
        updated_at: timestamp_to_rfc3339(order.updated_at),
        client_ip: order.client_ip,
        src_host: order.src_host,
        src_url: order.src_url,
        payment_success: derived.payment_success,
        recharge_success: derived.recharge_success,
        recharge_status: recharge_status_text(derived.recharge_status).to_string(),
        order_type: order.order_type,
        plan_id: order.plan_id,
        subscription_group_id: order.subscription_group_id,
        subscription_days: order.subscription_days,
        audit_logs: audit_logs.into_iter().map(to_audit_log_view).collect(),
    }
}

fn to_audit_log_view(item: AuditLogRecord) -> AuditLogView {
    AuditLogView {
        id: item.id,
        action: item.action,
        detail: item.detail,
        operator: item.operator,
        created_at: timestamp_to_rfc3339(item.created_at),
    }
}

fn recharge_status_text(status: RechargeStatus) -> &'static str {
    match status {
        RechargeStatus::NotPaid => "not_paid",
        RechargeStatus::PaidPending => "paid_pending",
        RechargeStatus::Recharging => "recharging",
        RechargeStatus::Success => "success",
        RechargeStatus::Failed => "failed",
        RechargeStatus::Closed => "closed",
    }
}

fn map_retry_error(error: anyhow::Error, locale: &str) -> AppError {
    match error.to_string().as_str() {
        "order not found" => AppError::not_found(message(locale, "订单不存在", "Order not found")),
        "order is not paid, retry denied" => AppError::bad_request(message(
            locale,
            "订单未支付，不允许重试",
            "Order is not paid, retry denied",
        )),
        "refund-related order cannot retry" => AppError::bad_request(message(
            locale,
            "退款相关订单不允许重试",
            "Refund-related order cannot retry",
        )),
        "order is recharging, retry later" => AppError::conflict(message(
            locale,
            "订单正在充值中，请稍后重试",
            "Order is recharging, retry later",
        )),
        "order already completed" => {
            AppError::bad_request(message(locale, "订单已完成", "Order already completed"))
        }
        "only paid and failed orders can retry" => AppError::bad_request(message(
            locale,
            "仅已支付和失败订单允许重试",
            "Only paid and failed orders can retry",
        )),
        "order status changed, refresh and retry" => AppError::conflict(message(
            locale,
            "订单状态已变更，请刷新后重试",
            "Order status changed, refresh and retry",
        )),
        _ => AppError::public_internal(message(locale, "重试充值失败", "Recharge retry failed")),
    }
}

fn map_cancel_error(error: anyhow::Error, locale: &str) -> AppError {
    match error.to_string().as_str() {
        "order not found" => AppError::not_found(message(locale, "订单不存在", "Order not found")),
        "order cannot be cancelled" => AppError::bad_request(message(
            locale,
            "订单当前状态不可取消",
            "Order cannot be cancelled",
        )),
        _ => AppError::public_internal(message(locale, "取消订单失败", "Cancel order failed")),
    }
}

fn total_pages(total: i64, page_size: i64) -> i64 {
    if total <= 0 || page_size <= 0 {
        0
    } else {
        (total + page_size - 1) / page_size
    }
}

fn parse_query_timestamp(value: &str, end_of_day: bool) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(timestamp) = trimmed.parse::<i64>() {
        return Some(timestamp);
    }

    if let Ok(datetime) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(datetime.timestamp());
    }

    let date = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()?;
    let naive = if end_of_day {
        date.and_hms_opt(23, 59, 59)?
    } else {
        date.and_hms_opt(0, 0, 0)?
    };
    Some(Utc.from_utc_datetime(&naive).timestamp())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{extract::State, http::HeaderMap};
    use uuid::Uuid;

    use super::*;
    use crate::{
        AppState,
        config::AppConfig,
        db::DatabaseHandle,
        order::{
            audit::AuditLogRepository,
            repository::{NewPendingOrder, OrderRepository},
            service::OrderService,
        },
        subscription_plan::SubscriptionPlanRepository,
        system_config::SystemConfigService,
    };

    async fn test_state() -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "opay-admin-orders-route-{}.db",
            Uuid::new_v4()
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
            platform_base_url: None,
            platform_timeout_secs: 2,
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
            platform: None,
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

    fn admin_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-admin-token".parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn list_orders_filters_and_exposes_retryable_flag() {
        let state = test_state().await;
        let repo = OrderRepository::new(state.db.clone());

        let first = repo
            .insert_pending(NewPendingOrder {
                user_id: 11,
                amount_cents: 1000,
                pay_amount_cents: Some(1100),
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
        repo.mark_paid_if_pending_or_recent_expired(crate::order::repository::MarkPaidInput {
            order_id: first.id.clone(),
            trade_no: "pi_test_1".to_string(),
            paid_amount_cents: 1100,
            paid_at: 2000,
            grace_updated_at_gte: 0,
        })
        .await
        .unwrap();
        repo.mark_recharging_if_paid_or_failed(&first.id)
            .await
            .unwrap();
        repo.mark_failed_if_recharging(&first.id, "recharge failed", 3000)
            .await
            .unwrap();

        repo.insert_pending(NewPendingOrder {
            user_id: 12,
            amount_cents: 500,
            pay_amount_cents: Some(500),
            fee_rate_bps: Some(0),
            status: "COMPLETED".to_string(),
            payment_type: "alipay".to_string(),
            order_type: "subscription".to_string(),
            plan_id: Some("plan_1".to_string()),
            subscription_group_id: Some(8),
            subscription_days: Some(30),
            provider_instance_id: None,
            expires_at: 1000,
        })
        .await
        .unwrap();

        let response = list_orders(
            State(state),
            admin_headers(),
            Query(AdminOrdersQuery {
                token: Some("test-admin-token".to_string()),
                lang: None,
                page: Some(1),
                page_size: Some(20),
                status: Some("FAILED".to_string()),
                order_type: Some("balance".to_string()),
                user_id: None,
                date_from: None,
                date_to: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.total, 1);
        assert_eq!(response.orders.len(), 1);
        assert!(response.orders[0].recharge_retryable);
        assert_eq!(response.orders[0].status, "FAILED");
    }

    #[tokio::test]
    async fn get_order_returns_detail_and_audit_logs() {
        let state = test_state().await;
        let repo = OrderRepository::new(state.db.clone());
        let audits = AuditLogRepository::new(state.db.clone());

        let order = repo
            .insert_pending(NewPendingOrder {
                user_id: 15,
                amount_cents: 900,
                pay_amount_cents: Some(990),
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
        repo.mark_paid_if_pending_or_recent_expired(crate::order::repository::MarkPaidInput {
            order_id: order.id.clone(),
            trade_no: "pi_test_detail".to_string(),
            paid_amount_cents: 990,
            paid_at: 2000,
            grace_updated_at_gte: 0,
        })
        .await
        .unwrap();
        repo.mark_completed_after_fulfillment(&order.id, 3000)
            .await
            .unwrap();
        audits
            .append(crate::order::audit::NewAuditLog {
                order_id: order.id.clone(),
                action: "ORDER_CREATED".to_string(),
                detail: Some("created".to_string()),
                operator: Some("user:15".to_string()),
            })
            .await
            .unwrap();

        let response = get_order(
            State(state),
            admin_headers(),
            Query(AdminOrdersQuery {
                token: Some("test-admin-token".to_string()),
                lang: None,
                page: None,
                page_size: None,
                status: None,
                order_type: None,
                user_id: None,
                date_from: None,
                date_to: None,
            }),
            Path(order.id),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.status, "COMPLETED");
        assert!(response.payment_success);
        assert_eq!(response.recharge_status, "success");
        assert_eq!(response.audit_logs.len(), 1);
        assert_eq!(response.audit_logs[0].action, "ORDER_CREATED");
    }
}
