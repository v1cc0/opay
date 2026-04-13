use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::post,
};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::{
    AppState,
    admin_auth::{AdminTokenQuery, verify_admin},
    error::{AppError, AppResult},
    http::common::{cents_to_amount, is_false, message, resolve_locale},
    order::service::{ProcessRefundInput, ProcessRefundResult},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/refund", post(post_refund))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefundResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    require_force: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    balance_deducted: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscription_days_deducted: Option<i64>,
}

async fn post_refund(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<AdminTokenQuery>,
    Json(body): Json<JsonValue>,
) -> AppResult<Json<RefundResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    verify_admin(&headers, query, &state).await?;

    let object = body.as_object().ok_or_else(|| invalid_parameters(locale))?;

    let order_id = object
        .get("order_id")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_parameters(locale))?
        .to_string();

    let amount_cents = match object.get("amount") {
        Some(JsonValue::Null) | None => None,
        Some(value) => {
            let amount = value
                .as_f64()
                .filter(|item| item.is_finite() && *item > 0.0);
            Some(
                amount
                    .map(crate::http::common::amount_to_cents)
                    .ok_or_else(|| invalid_parameters(locale))?,
            )
        }
    };

    let reason = match object.get("reason") {
        Some(JsonValue::Null) | None => None,
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => return Err(invalid_parameters(locale)),
    };

    let force = parse_optional_bool(object.get("force"), false, locale)?;
    let deduct_balance = parse_optional_bool(object.get("deduct_balance"), true, locale)?;

    let result = state
        .order_service
        .process_refund(
            &state,
            ProcessRefundInput {
                order_id,
                amount_cents,
                reason,
                force,
                deduct_balance,
            },
        )
        .await
        .map_err(|error| map_process_refund_error(error, locale))?;

    Ok(Json(to_response(locale, result)))
}

fn parse_optional_bool(value: Option<&JsonValue>, default: bool, locale: &str) -> AppResult<bool> {
    match value {
        None | Some(JsonValue::Null) => Ok(default),
        Some(JsonValue::Bool(value)) => Ok(*value),
        _ => Err(invalid_parameters(locale)),
    }
}

fn to_response(locale: &str, result: ProcessRefundResult) -> RefundResponse {
    let warning = result
        .warning
        .as_deref()
        .map(|value| localize_refund_warning(locale, value));

    if result.success {
        return RefundResponse {
            success: true,
            warning,
            require_force: false,
            balance_deducted: Some(cents_to_amount(result.balance_deducted_cents)),
            subscription_days_deducted: Some(result.subscription_days_deducted),
        };
    }

    RefundResponse {
        success: false,
        warning,
        require_force: result.require_force,
        balance_deducted: None,
        subscription_days_deducted: None,
    }
}

fn localize_refund_warning(locale: &str, warning: &str) -> String {
    match warning {
        "cannot fetch subscription info, use force" => message(
            locale,
            "无法获取订阅信息，请勾选强制退款",
            "Cannot fetch subscription info, use force",
        )
        .to_string(),
        "cannot fetch user balance, use force" => message(
            locale,
            "无法获取用户余额，请勾选强制退款",
            "Cannot fetch user balance, use force",
        )
        .to_string(),
        _ if warning.starts_with("gateway refund failed: ")
            && warning.ends_with(", deduction rolled back") =>
        {
            let reason = warning
                .trim_start_matches("gateway refund failed: ")
                .trim_end_matches(", deduction rolled back");
            if locale == "en" {
                warning.to_string()
            } else {
                format!("支付网关退款失败：{}，已回滚扣减", reason)
            }
        }
        _ => warning.to_string(),
    }
}

fn map_process_refund_error(error: anyhow::Error, locale: &str) -> AppError {
    match error.to_string().as_str() {
        "order not found" => AppError::not_found(message(locale, "订单不存在", "Order not found")),
        "order is not refundable in current status" => AppError::bad_request(message(
            locale,
            "仅已完成、已申请退款或退款失败的订单允许退款",
            "Only completed, refund-requested, or refund-failed orders can be refunded",
        )),
        "refund amount must be positive" => AppError::bad_request(message(
            locale,
            "退款金额必须大于 0",
            "Refund amount must be greater than 0",
        )),
        "refund amount exceeds recharge amount" => AppError::bad_request(message(
            locale,
            "退款金额不能超过充值金额",
            "Refund amount cannot exceed recharge amount",
        )),
        "order status changed, refresh and retry" => AppError::conflict(message(
            locale,
            "订单状态已变更，请刷新后重试",
            "Order status changed, refresh and retry",
        )),
        _ => AppError::public_internal(message(locale, "退款失败", "Refund failed")),
    }
}

fn invalid_parameters(locale: &str) -> AppError {
    AppError::bad_request(message(locale, "参数错误", "Invalid parameters"))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{Json, extract::State, http::HeaderMap};
    use serde_json::json;
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
        sub2api::Sub2ApiClient,
        subscription_plan::SubscriptionPlanRepository,
        system_config::{SystemConfigService, UpsertSystemConfig},
    };

    async fn test_state(sub2api_base_url: Option<String>) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "sub2apipay-admin-refund-route-{}.db",
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
            sub2api_base_url: sub2api_base_url.clone(),
            sub2api_timeout_secs: 2,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(1));
        let sub2api = sub2api_base_url.map(|base_url| Sub2ApiClient::new(base_url, 2));

        AppState {
            config: Arc::clone(&config),
            db: db.clone(),
            system_config: system_config.clone(),
            sub2api: sub2api.clone(),
            order_service: OrderService::new(
                Arc::clone(&config),
                OrderRepository::new(db.clone()),
                AuditLogRepository::new(db.clone()),
                SubscriptionPlanRepository::new(db.clone()),
                system_config,
                sub2api,
            ),
        }
    }

    fn admin_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-admin-token".parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn route_refunds_completed_order_without_gateway() {
        let state = test_state(Some("http://127.0.0.1:9".to_string())).await;
        let orders = OrderRepository::new(state.db.clone());

        state
            .system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 7,
                amount_cents: 1200,
                pay_amount_cents: Some(1299),
                fee_rate_bps: Some(825),
                status: "COMPLETED".to_string(),
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

        let response = post_refund(
            State(state.clone()),
            admin_headers(),
            Query(AdminTokenQuery::default()),
            Json(json!({
                "order_id": order.id,
                "deduct_balance": false
            })),
        )
        .await
        .unwrap()
        .0;

        assert!(response.success);
        assert_eq!(response.balance_deducted, Some(0.0));
        assert_eq!(response.subscription_days_deducted, Some(0));

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "REFUNDED");
        assert_eq!(saved.refund_amount_cents, Some(1200));
    }

    #[tokio::test]
    async fn route_rejects_empty_order_id() {
        let state = test_state(Some("http://127.0.0.1:9".to_string())).await;

        let error = post_refund(
            State(state),
            admin_headers(),
            Query(AdminTokenQuery::default()),
            Json(json!({
                "order_id": ""
            })),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, AppError::BadRequest(_)));
    }
}
